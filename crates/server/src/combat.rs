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
    let yaw = player.state().yaw;
    shared.broadcast_except(player.entity_id, cb::hurt_animation(player.entity_id, yaw));
    shared.broadcast_except(player.entity_id, cb::entity_status(player.entity_id, 2));

    if just_died {
        let msg = text::plain(format!("{} {}", player.name, cause));
        player.send(cb::death_combat_event(player.entity_id, &msg));
        shared.broadcast(cb::system_chat(&text::colored(format!("{} {}", player.name, cause), "red"), false));
    }
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
