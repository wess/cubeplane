//! Mob spawning and a lightweight AI tick: gravity, wandering, hostile chasing
//! and melee, plus death animations and despawning.

use std::f64::consts::TAU;
use std::sync::Arc;

use rand::Rng;

use cubeplane_world::block;

use crate::clientbound as cb;
use crate::combat;
use crate::entity::{Mob, MobKind};
use crate::player::Player;
use crate::state::Shared;

/// Detection radius (blocks) within which a hostile mob chases a player.
const AGGRO_RANGE: f64 = 16.0;
/// Despawn a mob this far from every player.
const DESPAWN_RANGE: f64 = 56.0;
/// Velocity unit conversion: blocks/tick → protocol (1/8000 block/tick).
const VEL_UNIT: f64 = 8000.0;

/// Advance all mobs by one tick and occasionally spawn new ones.
pub fn tick(shared: &Arc<Shared>, tick: u64) {
    let players = shared.players();

    // With nobody online, clear the world of mobs.
    if players.is_empty() {
        let ids: Vec<i32> = shared.mobs().iter().map(|m| m.entity_id).collect();
        if !ids.is_empty() {
            shared.mobs.write().unwrap().clear();
            shared.broadcast(cb::remove_entities(&ids));
        }
        return;
    }

    let mut remove = Vec::new();
    {
        let mut guard = shared.mobs.write().unwrap();
        for mob in guard.values_mut() {
            // Death animation countdown.
            if let Some(timer) = mob.dying.as_mut() {
                if *timer == 0 {
                    remove.push(mob.entity_id);
                } else {
                    *timer -= 1;
                }
                continue;
            }

            // Killed (by a player attack) but not yet animating.
            if mob.health <= 0.0 {
                mob.dying = Some(10);
                shared.broadcast(cb::entity_status(mob.entity_id, 3));
                continue;
            }

            // Despawn if far from everyone.
            if nearest_player(&players, mob.x, mob.z).map(|(_, d)| d).unwrap_or(f64::MAX)
                > DESPAWN_RANGE
            {
                remove.push(mob.entity_id);
                continue;
            }

            step_mob(shared, &players, mob, tick);
        }
        for id in &remove {
            guard.remove(id);
        }
    }
    if !remove.is_empty() {
        shared.broadcast(cb::remove_entities(&remove));
    }

    // Spawn roughly once a second.
    if tick.is_multiple_of(20) {
        try_spawn(shared, &players);
    }
}

/// Advance a single mob: movement, gravity, melee and broadcast.
fn step_mob(shared: &Arc<Shared>, players: &[Player], mob: &mut Mob, tick: u64) {
    let speed = mob.kind.speed();
    let target = if mob.kind.hostile() {
        nearest_player(players, mob.x, mob.z).filter(|(_, d)| *d <= AGGRO_RANGE)
    } else {
        None
    };

    // Decide horizontal movement.
    let (mut nx, mut nz) = (mob.x, mob.z);
    if let Some((p, _)) = &target {
        let s = p.state();
        let dx = s.x - mob.x;
        let dz = s.z - mob.z;
        let len = (dx * dx + dz * dz).sqrt().max(1e-6);
        nx += dx / len * speed;
        nz += dz / len * speed;
        mob.yaw = dz.atan2(dx).to_degrees() as f32; // face the target
    } else {
        if tick.is_multiple_of(60) {
            mob.heading = rand::thread_rng().gen::<f32>() * std::f32::consts::TAU;
        }
        nx += (mob.heading as f64).cos() * speed * 0.5;
        nz += (mob.heading as f64).sin() * speed * 0.5;
        mob.yaw = mob.heading.to_degrees();
    }

    // Collision + gravity against the world.
    {
        let mut world = shared.world.lock().unwrap();
        let mut solid = |x: f64, y: f64, z: f64| {
            !block::is_air(world.get_block(x.floor() as i32, y.floor() as i32, z.floor() as i32))
        };
        let foot = mob.y;
        if solid(nx, foot, nz) {
            // Try to step up one block.
            if !solid(nx, foot + 1.0, nz) && !solid(nx, foot + 2.0, nz) {
                mob.y += 1.0;
                mob.x = nx;
                mob.z = nz;
            }
        } else {
            mob.x = nx;
            mob.z = nz;
        }

        // Gravity: fall until something solid is underfoot.
        if solid(mob.x, mob.y - 0.1, mob.z) {
            mob.on_ground = true;
            mob.y = mob.y.floor();
        } else {
            mob.on_ground = false;
            mob.y -= 0.5;
            if mob.y < cubeplane_world::chunk::MIN_Y as f64 {
                mob.y = cubeplane_world::chunk::MIN_Y as f64;
                mob.on_ground = true;
            }
        }
    }

    // Melee.
    if mob.attack_cooldown > 0 {
        mob.attack_cooldown -= 1;
    }
    if mob.kind.attack_damage() > 0.0 {
        if let Some((p, _)) = &target {
            let s = p.state();
            let dx = s.x - mob.x;
            let dz = s.z - mob.z;
            let dist2 = dx * dx + dz * dz;
            if dist2 < 2.25 && (s.y - mob.y).abs() < 2.0 && mob.attack_cooldown == 0 && !s.dead {
                combat::damage_player(
                    shared,
                    p,
                    mob.kind.attack_damage(),
                    &format!("was slain by a {}", mob.kind.name()),
                );
                mob.attack_cooldown = 20;
                // Knock the player back.
                let len = dist2.sqrt().max(1e-6);
                let vx = (dx / len * 0.45 * VEL_UNIT) as i16;
                let vz = (dz / len * 0.45 * VEL_UNIT) as i16;
                p.send(cb::entity_velocity(p.entity_id, vx, 3500, vz));
            }
        }
    }

    // Broadcast the mob's new pose.
    shared.broadcast(cb::entity_teleport(mob.entity_id, mob.x, mob.y, mob.z, mob.yaw, mob.pitch, mob.on_ground));
    shared.broadcast(cb::entity_head_rotation(mob.entity_id, mob.yaw));
}

/// Try to spawn one mob near a random player, on the surface.
fn try_spawn(shared: &Arc<Shared>, players: &[Player]) {
    let cap = (players.len() * 8).min(40);
    if shared.mob_count() >= cap {
        return;
    }

    let mut rng = rand::thread_rng();
    let anchor = players[rng.gen_range(0..players.len())].state();
    let angle = rng.gen::<f64>() * TAU;
    let radius = rng.gen_range(10.0..18.0);
    let sx = anchor.x + angle.cos() * radius;
    let sz = anchor.z + angle.sin() * radius;
    let bx = sx.floor() as i32;
    let bz = sz.floor() as i32;

    // Find an open surface column.
    let spawn_y = {
        let mut world = shared.world.lock().unwrap();
        let top = (anchor.y as i32 + 24).min(cubeplane_world::chunk::MIN_Y + cubeplane_world::chunk::WORLD_HEIGHT - 3);
        let mut found = None;
        for y in (cubeplane_world::chunk::MIN_Y + 1..top).rev() {
            let ground = !block::is_air(world.get_block(bx, y, bz));
            let head_clear = block::is_air(world.get_block(bx, y + 1, bz))
                && block::is_air(world.get_block(bx, y + 2, bz));
            if ground && head_clear {
                found = Some(y + 1);
                break;
            }
        }
        found
    };
    let Some(y) = spawn_y else {
        return;
    };

    let kind = MobKind::ALL[rng.gen_range(0..MobKind::ALL.len())];
    let entity_id = shared.next_entity_id();
    let heading = rng.gen::<f32>() * std::f32::consts::TAU;
    let mob = Mob::new(entity_id, kind, sx, y as f64, sz, heading);

    shared.broadcast(cb::spawn_entity(
        entity_id,
        mob.uuid,
        kind.type_id(),
        mob.x,
        mob.y,
        mob.z,
        mob.yaw,
        mob.pitch,
        mob.yaw,
        0,
        (0, 0, 0),
    ));
    shared.add_mob(mob);
}

/// Find the nearest living player to a point, with horizontal distance.
fn nearest_player(players: &[Player], x: f64, z: f64) -> Option<(Player, f64)> {
    players
        .iter()
        .filter(|p| !p.is_dead())
        .map(|p| {
            let s = p.state();
            let dx = s.x - x;
            let dz = s.z - z;
            (p.clone(), (dx * dx + dz * dz).sqrt())
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// Apply a player's melee attack to a mob (called from the connection task).
pub fn player_attack(shared: &Arc<Shared>, attacker: &Player, target: i32) {
    const FIST_DAMAGE: f32 = 3.0;
    let hit = shared.with_mob(target, |m| {
        if m.alive() {
            m.health -= FIST_DAMAGE;
            Some((m.x, m.y, m.z, m.health <= 0.0))
        } else {
            None
        }
    });

    let Some(Some((mx, _my, mz, killed))) = hit else {
        return;
    };

    shared.broadcast(cb::hurt_animation(target, 0.0));
    shared.broadcast(cb::entity_status(target, 2));

    // Knock the mob away from the attacker.
    let s = attacker.state();
    let dx = mx - s.x;
    let dz = mz - s.z;
    let len = (dx * dx + dz * dz).sqrt().max(1e-6);
    let vx = (dx / len * 0.5 * VEL_UNIT) as i16;
    let vz = (dz / len * 0.5 * VEL_UNIT) as i16;
    shared.broadcast(cb::entity_velocity(target, vx, 3200, vz));

    if killed {
        // Begin the death animation; the tick removes it shortly after.
        shared.with_mob(target, |m| {
            if m.dying.is_none() {
                m.dying = Some(10);
            }
        });
        shared.broadcast(cb::entity_status(target, 3));
    }
}
