//! Status-effect gameplay: regeneration, poison, strength, speed and the
//! instant health/damage effects, applied to players over time.
//!
//! Effects are stored per player in [`Shared`] and ticked once a second. Speed
//! adjusts the client's walk speed via Player Abilities; strength is read by
//! the melee path; regeneration/poison change health on each tick.

use std::sync::Arc;

use crate::clientbound as cb;
use crate::combat;
use crate::player::Player;
use crate::state::Shared;

// Common 1.20.1 effect ids.
pub const SPEED: i32 = 1;
pub const STRENGTH: i32 = 5;
pub const INSTANT_HEALTH: i32 = 6;
pub const INSTANT_DAMAGE: i32 = 7;
pub const REGENERATION: i32 = 10;
pub const POISON: i32 = 19;

/// One active effect on a player.
#[derive(Debug, Clone, Copy)]
pub struct ActiveEffect {
    pub id: i32,
    pub amplifier: i8,
    /// Remaining duration in ticks.
    pub ticks: u32,
}

/// Apply an effect to a player: register it, notify the client, and handle the
/// instant effects immediately.
pub fn apply(shared: &Arc<Shared>, player: &Player, id: i32, amplifier: i8, seconds: i32) {
    let level = amplifier.max(0) as f32 + 1.0;
    match id {
        INSTANT_HEALTH => {
            let heal = 4.0 * 2f32.powf(amplifier.max(0) as f32);
            combat::heal(player, heal);
            return;
        }
        INSTANT_DAMAGE => {
            combat::damage_player(shared, player, 6.0 * 2f32.powf(amplifier.max(0) as f32), "took a turn for the worse");
            return;
        }
        SPEED => set_walk_speed(player, 0.1 * (1.0 + 0.2 * level)),
        _ => {}
    }
    let ticks = (seconds.max(1) as u32) * 20;
    shared.add_effect(player.entity_id, ActiveEffect { id, amplifier, ticks });
    player.send(cb::entity_effect(player.entity_id, id, amplifier, ticks as i32, 0x02));
}

/// Per-second effect tick: regenerate/poison, expire effects.
pub fn tick(shared: &Arc<Shared>) {
    for player in shared.players() {
        let active = shared.player_effects(player.entity_id);
        if active.is_empty() {
            continue;
        }
        let mut expired = Vec::new();
        for e in &active {
            match e.id {
                REGENERATION => combat::heal(&player, 1.0 + e.amplifier.max(0) as f32),
                POISON => {
                    if player.state().health > 1.0 {
                        combat::damage_player(shared, &player, 1.0, "succumbed to poison");
                    }
                }
                _ => {}
            }
        }
        // Decrement durations and collect expirations.
        shared.update_effects(player.entity_id, |list| {
            list.retain_mut(|e| {
                e.ticks = e.ticks.saturating_sub(20);
                if e.ticks == 0 {
                    expired.push(e.id);
                    false
                } else {
                    true
                }
            });
        });
        for id in expired {
            player.send(cb::remove_entity_effect(player.entity_id, id));
            if id == SPEED {
                set_walk_speed(&player, 0.1);
            }
        }
    }
}

/// Send updated abilities with a new walk speed.
fn set_walk_speed(player: &Player, speed: f32) {
    let flags = if player.gamemode() == 1 { 0x0D } else { 0x00 };
    player.send(cb::player_abilities(flags, 0.05, speed));
}
