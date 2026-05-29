//! Dropped items and projectiles: spawning, physics, pickup and hit detection.

use std::sync::Arc;

use rand::Rng;
use uuid::Uuid;

use cubeplane_world::block;

use crate::clientbound as cb;
use crate::combat;
use crate::entity::{self, ItemEntity, Projectile};
use crate::state::Shared;

/// Ticks before a dropped item despawns (5 minutes at 20 TPS).
const ITEM_LIFETIME: u32 = 6000;
/// Horizontal pickup radius.
const PICKUP_RANGE: f64 = 1.3;

/// Drop an item stack into the world with a small random pop.
pub fn spawn_item(shared: &Arc<Shared>, item_id: i32, count: u8, x: f64, y: f64, z: f64, pickup_delay: u32) {
    if item_id == 0 || count == 0 {
        return;
    }
    let entity_id = shared.next_entity_id();
    let uuid = Uuid::new_v4();
    let mut rng = rand::thread_rng();
    let vel = (
        (rng.gen::<f64>() - 0.5) * 0.2 * 8000.0,
        2000.0,
        (rng.gen::<f64>() - 0.5) * 0.2 * 8000.0,
    );
    shared.broadcast(cb::spawn_entity(
        entity_id,
        uuid,
        entity::ITEM_ENTITY,
        x,
        y,
        z,
        0.0,
        0.0,
        0.0,
        1, // object data: 1 so the item renders immediately
        (vel.0 as i16, vel.1 as i16, vel.2 as i16),
    ));
    shared.broadcast(cb::entity_metadata_item(
        entity_id,
        crate::item::ItemStack::new(item_id, count),
    ));
    shared.add_item_entity(ItemEntity {
        entity_id,
        uuid,
        item_id,
        count,
        x,
        y,
        z,
        on_ground: false,
        age: 0,
        pickup_delay,
    });
}

/// Fire a projectile from `owner` toward a target point.
#[allow(clippy::too_many_arguments)]
pub fn spawn_arrow(shared: &Arc<Shared>, owner: i32, x: f64, y: f64, z: f64, dx: f64, dy: f64, dz: f64, damage: f32) {
    let entity_id = shared.next_entity_id();
    let len = (dx * dx + dy * dy + dz * dz).sqrt().max(1e-6);
    let speed = 1.6;
    let (vx, vy, vz) = (dx / len * speed, dy / len * speed, dz / len * speed);
    shared.broadcast(cb::spawn_entity(
        entity_id,
        Uuid::new_v4(),
        entity::ARROW,
        x,
        y,
        z,
        0.0,
        0.0,
        0.0,
        owner,
        ((vx * 8000.0) as i16, (vy * 8000.0) as i16, (vz * 8000.0) as i16),
    ));
    shared.add_projectile(Projectile { entity_id, x, y, z, vx, vy, vz, damage, age: 0, owner });
}

/// Advance item entities and projectiles by one tick.
pub fn tick(shared: &Arc<Shared>) {
    tick_items(shared);
    tick_projectiles(shared);
}

fn tick_items(shared: &Arc<Shared>) {
    let players = shared.players();
    let mut remove = Vec::new();
    let mut collected = Vec::new(); // (item_eid, collector_eid, item_id, count)

    {
        let mut guard = shared.items.write().unwrap();
        for item in guard.values_mut() {
            item.age += 1;
            if item.pickup_delay > 0 {
                item.pickup_delay -= 1;
            }
            if item.age > ITEM_LIFETIME {
                remove.push(item.entity_id);
                continue;
            }

            // Settle with gravity.
            let on_ground = {
                let mut world = shared.world.lock().unwrap();
                !block::is_air(world.get_block(
                    item.x.floor() as i32,
                    (item.y - 0.1).floor() as i32,
                    item.z.floor() as i32,
                ))
            };
            if on_ground {
                item.on_ground = true;
                item.y = item.y.floor() + 0.0;
            } else {
                item.y -= 0.4;
                if item.y < cubeplane_world::chunk::MIN_Y as f64 {
                    remove.push(item.entity_id);
                    continue;
                }
            }

            // Pickup.
            if item.pickup_delay == 0 {
                if let Some(p) = players.iter().filter(|p| !p.is_dead()).find(|p| {
                    let s = p.state();
                    let dx = s.x - item.x;
                    let dz = s.z - item.z;
                    let dy = s.y - item.y;
                    dx * dx + dz * dz <= PICKUP_RANGE * PICKUP_RANGE && dy.abs() < 2.0
                }) {
                    collected.push((item.entity_id, p.entity_id, item.item_id, item.count, p.clone()));
                    remove.push(item.entity_id);
                }
            }
        }
        for id in &remove {
            guard.remove(id);
        }
    }

    for (item_eid, collector, id, count, player) in collected {
        let leftover = player.inventory(|inv| inv.add(id, count));
        player.sync_inventory();
        shared.broadcast(cb::collect_item(item_eid, collector, (count - leftover) as i32));
        // If it didn't all fit, drop the remainder back (rare).
        if leftover > 0 {
            let s = player.state();
            spawn_item(shared, id, leftover, s.x, s.y, s.z, 20);
        }
    }
    if !remove.is_empty() {
        shared.broadcast(cb::remove_entities(&remove));
    }
}

fn tick_projectiles(shared: &Arc<Shared>) {
    let players = shared.players();
    let mut remove = Vec::new();
    let mut hits = Vec::new(); // (player, damage)

    {
        let mut guard = shared.projectiles.write().unwrap();
        for p in guard.values_mut() {
            p.age += 1;
            p.vy -= 0.05; // gravity
            p.x += p.vx;
            p.y += p.vy;
            p.z += p.vz;

            // Despawn after 5s or on hitting a block.
            let in_block = {
                let mut world = shared.world.lock().unwrap();
                !block::is_air(world.get_block(p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32))
            };
            if p.age > 100 || in_block || p.y < cubeplane_world::chunk::MIN_Y as f64 {
                remove.push(p.entity_id);
                continue;
            }

            // Hit a player?
            if let Some(player) = players.iter().filter(|pl| !pl.is_dead()).find(|pl| {
                let s = pl.state();
                let dx = s.x - p.x;
                let dy = (s.y + 1.0) - p.y;
                let dz = s.z - p.z;
                dx * dx + dy * dy + dz * dz < 1.2
            }) {
                hits.push((player.clone(), p.damage));
                remove.push(p.entity_id);
            } else {
                shared.broadcast(cb::entity_teleport(p.entity_id, p.x, p.y, p.z, 0.0, 0.0, false));
            }
        }
        for id in &remove {
            guard.remove(id);
        }
    }

    for (player, damage) in hits {
        combat::damage_player(shared, &player, damage, "was shot");
    }
    if !remove.is_empty() {
        shared.broadcast(cb::remove_entities(&remove));
    }
}
