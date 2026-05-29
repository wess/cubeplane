//! Shared combat helpers: applying damage to players, death and regeneration.
//!
//! These run from several places — the connection task (fall damage), the mob
//! AI tick (melee) and the game loop (regen) — so they take an `&Arc<Shared>`
//! plus a [`Player`] handle and operate through its locked state and sender.

use std::sync::Arc;

use crate::clientbound as cb;
use crate::player::{Player, MAX_HEALTH};
use crate::state::Shared;
use crate::text;

/// Apply `amount` half-hearts of damage to a player, handling HUD updates,
/// the hurt animation seen by others, and death.
pub fn damage_player(shared: &Arc<Shared>, player: &Player, amount: f32, cause: &str) {
    if amount <= 0.0 {
        return;
    }

    // Armor reduces incoming damage by 4% per point (capped at 80%), and the
    // Protection enchant adds ~4% per level on top (combined cap 85%).
    let (defense, prot) = player.inventory(|inv| (inv.armor_defense(), inv.protection_levels()));
    let reduction = (defense * 0.04 + prot as f32 * 0.04).min(0.85);
    let amount = amount * (1.0 - reduction);

    let (new_health, food, saturation, just_died) = player.update(|s| {
        if s.dead {
            return (s.health, s.food, s.saturation, false);
        }
        let before = s.health;
        s.health = (s.health - amount).max(0.0);
        let died = s.health <= 0.0 && before > 0.0;
        if died {
            s.dead = true;
        }
        (s.health, s.food, s.saturation, died)
    });

    if player.is_dead() && !just_died {
        return; // already dead; nothing to do
    }

    // Update the victim's HUD and flash everyone else's view of them.
    player.send(cb::update_health(new_health, food, saturation));
    let s = player.state();
    let yaw = s.yaw;
    shared.broadcast_except(player.entity_id, cb::hurt_animation(player.entity_id, yaw));
    shared.broadcast_except(player.entity_id, cb::entity_status(player.entity_id, 2));
    let sound = if just_died { "entity.player.death" } else { "entity.player.hurt" };
    shared.broadcast(cb::sound_effect(sound, 7, s.x, s.y, s.z, 1.0, 1.0));

    if just_died {
        let msg = text::plain(format!("{} {}", player.name, cause));
        player.send(cb::death_combat_event(player.entity_id, &msg));
        shared.broadcast(cb::system_chat(&text::colored(format!("{} {}", player.name, cause), "red"), false));

        // Big red "You Died" title for the victim.
        player.send(cb::title_times(5, 60, 20));
        player.send(cb::title_subtitle(&text::colored(cause, "gray")));
        player.send(cb::title_text(&text::colored("You Died", "red")));

        // Drop the player's items unless keepInventory is enabled.
        if !shared.config.world.keep_inventory {
            let s = player.state();
            let dropped: Vec<(i32, u8)> = player.inventory(|inv| {
                let list = inv
                    .slots()
                    .iter()
                    .filter(|st| !st.is_empty())
                    .map(|st| (st.id, st.count))
                    .collect();
                *inv = crate::inventory::Inventory::default();
                list
            });
            for (id, count) in dropped {
                crate::drops::spawn_item(shared, id, count, s.x, s.y + 0.5, s.z, 20);
            }
            player.sync_inventory();
        }
    }
}

/// Periodic hunger drain for survival players: burn saturation, then food,
/// then starve. Called every ~30 seconds from the game loop.
pub fn hunger_tick(shared: &Arc<Shared>) {
    for player in shared.players() {
        if player.is_dead() || player.gamemode() == 1 {
            continue;
        }
        let (health, food, saturation, starving) = player.update(|s| {
            if s.saturation > 0.0 {
                s.saturation = (s.saturation - 1.0).max(0.0);
            } else if s.food > 0 {
                s.food -= 1;
            }
            (s.health, s.food, s.saturation, s.food == 0)
        });
        player.send(cb::update_health(health, food, saturation));
        // Starvation damages down to (but not below) one heart.
        if starving && health > 2.0 {
            damage_player(shared, &player, 1.0, "starved to death");
        }
    }
}

/// Heal a living player by `amount` (clamped to full), updating their HUD.
pub fn heal(player: &Player, amount: f32) {
    let (h, f, sat) = player.update(|s| {
        if !s.dead {
            s.health = (s.health + amount).min(MAX_HEALTH);
        }
        (s.health, s.food, s.saturation)
    });
    player.send(cb::update_health(h, f, sat));
}

/// Grant experience and update the XP HUD. Levels use a simple linear curve.
pub fn grant_xp(player: &Player, amount: i32) {
    let total = player.update(|s| {
        s.xp_total = (s.xp_total + amount).max(0);
        s.xp_total
    });
    player.send(cb::set_experience(xp_bar(total), total / 10, total));
    let s = player.state();
    player.send(cb::sound_effect("entity.experience_orb.pickup", 7, s.x, s.y, s.z, 0.3, 1.0));
}

/// Fraction of the way to the next level for a given total (0.0..1.0).
pub fn xp_bar(total: i32) -> f32 {
    (total % 10) as f32 / 10.0
}

/// Reset a player to full health (used on respawn).
pub fn revive(player: &Player) {
    player.update(|s| {
        s.health = MAX_HEALTH;
        s.food = 20;
        s.saturation = 5.0;
        s.dead = false;
    });
    player.send(cb::update_health(MAX_HEALTH, 20, 5.0));
}

/// Slowly regenerate one half-heart for well-fed, living players. Called on a
/// timer from the game loop.
pub fn regenerate(shared: &Arc<Shared>) {
    for player in shared.players() {
        let healed = player.update(|s| {
            if !s.dead && s.food >= 18 && s.health < MAX_HEALTH {
                s.health = (s.health + 1.0).min(MAX_HEALTH);
                Some((s.health, s.food, s.saturation))
            } else {
                None
            }
        });
        if let Some((h, f, sat)) = healed {
            player.send(cb::update_health(h, f, sat));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::player::Player;
    use cubeplane_world::{FlatGenerator, World};
    use tokio::sync::mpsc::unbounded_channel;
    use uuid::Uuid;

    fn fixture() -> (Arc<Shared>, Player) {
        let shared = Shared::new(
            Config::default(),
            World::new(Arc::new(FlatGenerator::default())),
            None,
            None,
        );
        let (tx, _rx) = unbounded_channel();
        let player = Player::new(1, Uuid::nil(), "Tester".into(), 0, tx, (0.0, 64.0, 0.0));
        shared.add_player(player.clone());
        (shared, player)
    }

    #[test]
    fn damage_reduces_health_then_kills_then_revives() {
        let (shared, player) = fixture();
        damage_player(&shared, &player, 5.0, "hurt");
        assert_eq!(player.state().health, 15.0);
        assert!(!player.is_dead());

        damage_player(&shared, &player, 100.0, "died");
        assert!(player.is_dead());
        assert_eq!(player.state().health, 0.0);

        // A dead player takes no further damage.
        damage_player(&shared, &player, 5.0, "again");
        assert_eq!(player.state().health, 0.0);

        revive(&player);
        assert!(!player.is_dead());
        assert_eq!(player.state().health, MAX_HEALTH);
    }

    #[test]
    fn regen_heals_when_fed() {
        let (shared, player) = fixture();
        player.update(|s| s.health = 10.0);
        regenerate(&shared);
        assert_eq!(player.state().health, 11.0);
    }
}
