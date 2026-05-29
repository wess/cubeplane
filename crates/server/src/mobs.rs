//! Mob spawning and a lightweight AI tick: gravity, wandering, hostile chasing
//! and melee, plus death animations and despawning.

use std::f64::consts::TAU;
use std::sync::Arc;

use rand::Rng;

use cubeplane_world::block;

use crate::clientbound as cb;
use crate::combat;
use crate::drops;
use crate::entity::{Mob, MobKind};
use crate::item;
use crate::player::Player;
use crate::state::Shared;
use crate::text;

/// Register an AI villager's personality and broadcast its nameplate.
fn villager_spawned(shared: &Arc<Shared>, entity_id: i32) {
    shared.register_villager(entity_id);
    if shared.ai_config().enabled {
        if let Some((name, prof)) = shared.villager_identity(entity_id) {
            let label = text::colored(format!("{name} the {prof}"), "green");
            shared.broadcast(cb::entity_custom_name(entity_id, &label));
        }
    }
}

/// Detection radius (blocks) within which a hostile mob chases a player.
const AGGRO_RANGE: f64 = 16.0;
/// Despawn a mob this far from every player.
const DESPAWN_RANGE: f64 = 56.0;
/// Velocity unit conversion: blocks/tick → protocol (1/8000 block/tick).
const VEL_UNIT: f64 = 8000.0;

/// Advance all mobs by one tick and occasionally spawn new ones.
pub fn tick(shared: &Arc<Shared>, tick: u64, is_night: bool) {
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
    let mut lovers: Vec<(i32, MobKind, f64, f64, f64)> = Vec::new();
    let mut babies: Vec<(MobKind, f64, f64, f64)> = Vec::new();
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

            // Killed (by a player attack) but not yet animating: drop loot,
            // award XP and play the death animation.
            if mob.health <= 0.0 {
                mob.dying = Some(10);
                shared.broadcast(cb::entity_status(mob.entity_id, 3));
                drop_loot(shared, mob.kind, mob.x, mob.y, mob.z);
                let xp = xp_for(mob.kind);
                shared.broadcast(cb::spawn_xp_orb(shared.next_entity_id(), mob.x, mob.y, mob.z, xp as i16));
                if let Some((p, _)) = nearest_player(&players, mob.x, mob.z) {
                    combat::grant_xp(&p, xp);
                }
                continue;
            }

            // Despawn if far from everyone.
            if nearest_player(&players, mob.x, mob.z).map(|(_, d)| d).unwrap_or(f64::MAX)
                > DESPAWN_RANGE
            {
                remove.push(mob.entity_id);
                continue;
            }

            if mob.in_love > 0 {
                mob.in_love -= 1;
                lovers.push((mob.entity_id, mob.kind, mob.x, mob.y, mob.z));
            }
            step_mob(shared, &players, mob, tick);
        }
        for id in &remove {
            guard.remove(id);
        }

        // Breed: pair up nearby in-love mobs of the same kind.
        let mut used = std::collections::HashSet::new();
        for i in 0..lovers.len() {
            let (ea, ka, xa, ya, za) = lovers[i];
            if used.contains(&ea) {
                continue;
            }
            for &(eb, kb, xb, _, zb) in lovers.iter().skip(i + 1) {
                if used.contains(&eb) || ka != kb {
                    continue;
                }
                if (xa - xb).powi(2) + (za - zb).powi(2) < 64.0 {
                    used.insert(ea);
                    used.insert(eb);
                    if let Some(m) = guard.get_mut(&ea) {
                        m.in_love = 0;
                    }
                    if let Some(m) = guard.get_mut(&eb) {
                        m.in_love = 0;
                    }
                    babies.push((ka, (xa + xb) / 2.0, ya, (za + zb) / 2.0));
                    break;
                }
            }
        }
    }

    for (kind, x, y, z) in babies {
        spawn_baby(shared, kind, x, y, z);
    }
    if !remove.is_empty() {
        for id in &remove {
            shared.remove_villager(*id);
        }
        shared.broadcast(cb::remove_entities(&remove));
    }

    // Spawn roughly once a second.
    if tick.is_multiple_of(20) {
        try_spawn(shared, &players, is_night);
    }
}

/// XP awarded for killing a mob.
fn xp_for(kind: MobKind) -> i32 {
    if kind.hostile() {
        5
    } else {
        2
    }
}

/// Drop a mob's loot at its position.
fn drop_loot(shared: &Arc<Shared>, kind: MobKind, x: f64, y: f64, z: f64) {
    let mut rng = rand::thread_rng();
    let mut drop = |name: &str, lo: u8, hi: u8| {
        if let Some(id) = item::by_name(name) {
            let count = rng.gen_range(lo..=hi);
            if count > 0 {
                drops::spawn_item(shared, id, count, x, y + 0.3, z, 10);
            }
        }
    };
    match kind.name() {
        "zombie" | "husk" => drop("rotten_flesh", 0, 2),
        "skeleton" | "stray" => {
            drop("bone", 0, 2);
            drop("arrow", 0, 2);
        }
        "spider" | "cave_spider" => {
            drop("string", 0, 2);
            drop("spider_eye", 0, 1);
        }
        "creeper" => drop("gunpowder", 0, 2),
        "pig" => drop("porkchop", 1, 3),
        "sheep" => drop("mutton", 1, 2),
        "chicken" => {
            drop("feather", 0, 2);
            drop("egg", 0, 1);
        }
        _ => {}
    }
}

/// Blow up at `(cx,cy,cz)`: clear blocks in a sphere, hurt nearby players, and
/// send the Explosion packet (which drives the client's particles and sound).
fn explode(shared: &Arc<Shared>, cx: f64, cy: f64, cz: f64, power: f32) {
    let r = power.ceil() as i32;
    let (bx, by, bz) = (cx.floor() as i32, cy.floor() as i32, cz.floor() as i32);
    let mut offsets = Vec::new();
    {
        let mut world = shared.world.lock().unwrap();
        for dy in -r..=r {
            for dz in -r..=r {
                for dx in -r..=r {
                    if ((dx * dx + dy * dy + dz * dz) as f32).sqrt() > power {
                        continue;
                    }
                    let (wx, wy, wz) = (bx + dx, by + dy, bz + dz);
                    let cur = world.get_block(wx, wy, wz);
                    if cur != block::AIR && cur != block::BEDROCK {
                        world.set_block(wx, wy, wz, block::AIR);
                        offsets.push((dx as i8, dy as i8, dz as i8));
                    }
                }
            }
        }
    }
    shared.broadcast(cb::explosion(cx, cy, cz, power, &offsets));

    // Hurt players within the blast.
    for p in shared.players() {
        if p.is_dead() {
            continue;
        }
        let s = p.state();
        let dist = ((s.x - cx).powi(2) + (s.y - cy).powi(2) + (s.z - cz).powi(2)).sqrt();
        if dist <= power as f64 * 1.5 {
            let dmg = ((1.0 - dist / (power as f64 * 1.5)) * 14.0).max(1.0) as f32;
            combat::damage_player(shared, &p, dmg, "was blown up");
        }
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

    // Attacks.
    if mob.attack_cooldown > 0 {
        mob.attack_cooldown -= 1;
    }
    if let Some((p, dist)) = &target {
        let s = p.state();
        let dx = s.x - mob.x;
        let dz = s.z - mob.z;
        let dist2 = dx * dx + dz * dz;

        let name = mob.kind.name();
        if name == "creeper" && *dist < 3.0 {
            // Creepers detonate on contact.
            explode(shared, mob.x, mob.y, mob.z, 3.0);
            mob.health = 0.0; // removed (with loot) next tick
            return;
        } else if matches!(name, "skeleton" | "stray")
            && *dist < 14.0
            && mob.attack_cooldown == 0
            && !s.dead
        {
            // Skeletons fire arrows from range.
            drops::spawn_arrow(
                shared,
                mob.entity_id,
                mob.x,
                mob.y + 1.2,
                mob.z,
                dx,
                (s.y + 1.0) - (mob.y + 1.2),
                dz,
                2.0,
            );
            mob.attack_cooldown = 40;
        } else if mob.kind.attack_damage() > 0.0
            && dist2 < 2.25
            && (s.y - mob.y).abs() < 2.0
            && mob.attack_cooldown == 0
            && !s.dead
        {
            // Melee mobs (zombies, spiders, …).
            combat::damage_player(
                shared,
                p,
                mob.kind.attack_damage(),
                &format!("was slain by a {}", mob.kind.name()),
            );
            mob.attack_cooldown = 20;
            let len = dist2.sqrt().max(1e-6);
            let vx = (dx / len * 0.45 * VEL_UNIT) as i16;
            let vz = (dz / len * 0.45 * VEL_UNIT) as i16;
            p.send(cb::entity_velocity(p.entity_id, vx, 3500, vz));
        }
    }

    // Broadcast the mob's new pose.
    shared.broadcast(cb::entity_teleport(mob.entity_id, mob.x, mob.y, mob.z, mob.yaw, mob.pitch, mob.on_ground));
    shared.broadcast(cb::entity_head_rotation(mob.entity_id, mob.yaw));
}

/// Try to spawn one mob near a random player, on the surface. Hostiles only
/// appear at night (and only if enabled in config).
fn try_spawn(shared: &Arc<Shared>, players: &[Player], is_night: bool) {
    let cap = (players.len() * 8).min(40);
    if shared.mob_count() >= cap {
        return;
    }
    let allow_hostiles = is_night && shared.config.world.spawn_hostiles;

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

    // Pick a kind; restrict to passive animals when hostiles aren't allowed.
    let kind = MobKind::random(&mut rng, !allow_hostiles);
    let entity_id = shared.next_entity_id();
    let heading = rng.gen::<f32>() * std::f32::consts::TAU;
    let mut mob = Mob::new(entity_id, kind, sx, y as f64, sz, heading);
    if kind.name() == "sheep" {
        mob.variant = rng.gen_range(0..16);
    }

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
    let meta = mob.metadata();
    if !meta.is_empty() {
        shared.broadcast(cb::entity_metadata(entity_id, &meta));
    }
    shared.add_mob(mob);
    if kind.name() == "villager" {
        villager_spawned(shared, entity_id);
    }
}

/// Spawn a baby animal (from breeding) at a position.
fn spawn_baby(shared: &Arc<Shared>, kind: MobKind, x: f64, y: f64, z: f64) {
    let entity_id = shared.next_entity_id();
    let heading = rand::thread_rng().gen::<f32>() * std::f32::consts::TAU;
    let mut mob = Mob::new(entity_id, kind, x, y, z, heading);
    mob.baby = true;
    shared.broadcast(cb::spawn_entity(
        entity_id, mob.uuid, kind.type_id(), x, y, z, mob.yaw, mob.pitch, mob.yaw, 0, (0, 0, 0),
    ));
    let meta = mob.metadata();
    if !meta.is_empty() {
        shared.broadcast(cb::entity_metadata(entity_id, &meta));
    }
    shared.add_mob(mob);
}

/// Spawn a specific mob at a position (used by the /summon command and mods).
pub fn summon(shared: &Arc<Shared>, kind: MobKind, x: f64, y: f64, z: f64) {
    let entity_id = shared.next_entity_id();
    let heading = rand::thread_rng().gen::<f32>() * std::f32::consts::TAU;
    let mob = Mob::new(entity_id, kind, x, y, z, heading);
    shared.broadcast(cb::spawn_entity(
        entity_id, mob.uuid, kind.type_id(), mob.x, mob.y, mob.z, mob.yaw, mob.pitch, mob.yaw, 0, (0, 0, 0),
    ));
    shared.add_mob(mob);
    if kind.name() == "villager" {
        villager_spawned(shared, entity_id);
    }
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
/// Death (loot/XP/animation) is handled centrally by [`tick`].
pub fn player_attack(shared: &Arc<Shared>, attacker: &Player, target: i32, damage: f32) {
    let hit = shared.with_mob(target, |m| {
        if m.alive() {
            m.health -= damage;
            Some((m.x, m.z))
        } else {
            None
        }
    });

    let Some(Some((mx, mz))) = hit else {
        return;
    };

    shared.broadcast(cb::hurt_animation(target, 0.0));
    shared.broadcast(cb::entity_status(target, 2));
    shared.broadcast(cb::sound_effect("entity.generic.hurt", 6, mx, attacker.state().y, mz, 1.0, 1.0));

    // Knock the mob away from the attacker.
    let s = attacker.state();
    let dx = mx - s.x;
    let dz = mz - s.z;
    let len = (dx * dx + dz * dz).sqrt().max(1e-6);
    let vx = (dx / len * 0.5 * VEL_UNIT) as i16;
    let vz = (dz / len * 0.5 * VEL_UNIT) as i16;
    shared.broadcast(cb::entity_velocity(target, vx, 3200, vz));
}
