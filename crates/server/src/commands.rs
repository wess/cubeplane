//! Slash-command handling: built-in player commands plus op-gated "cheat"
//! commands (gamemode, give, time, weather, summon, effect, …).
//!
//! [`dispatch`] returns `true` if it handled the command; the caller forwards
//! anything unhandled to the mod runtime so JS commands still work.

use std::sync::Arc;

use crate::clientbound as cb;
use crate::combat;
use crate::entity::{MobKind, Vehicle};
use crate::inventory::Inventory;
use crate::item;
use crate::mobs;
use crate::player::Player;
use crate::state::Shared;
use crate::text;

/// Whether `name` may run cheat commands (empty op list ⇒ everyone can).
pub fn is_op(shared: &Arc<Shared>, name: &str) -> bool {
    let ops = &shared.config.server.ops;
    ops.is_empty() || ops.iter().any(|o| o.eq_ignore_ascii_case(name))
}

fn tell(player: &Player, msg: impl Into<String>, color: &str) {
    player.send(cb::system_chat(&text::colored(msg, color), false));
}

/// Dispatch a slash command. Returns `true` if handled.
pub fn dispatch(shared: &Arc<Shared>, player: &Player, name: &str, args: &[String]) -> bool {
    let op = is_op(shared, &player.name);
    match name {
        "help" => {
            tell(
                player,
                "Commands: /help /list /pos /tp /xp · op: /gamemode /give /time /weather /summon /effect /heal /kill /clear",
                "aqua",
            );
        }
        "list" => {
            let names: Vec<String> = shared.players().iter().map(|p| p.name.clone()).collect();
            tell(player, format!("{} online: {}", names.len(), names.join(", ")), "aqua");
        }
        "pos" => {
            let s = player.state();
            tell(player, format!("x={:.1} y={:.1} z={:.1}", s.x, s.y, s.z), "aqua");
        }
        "tp" => teleport(shared, player, args),
        "gamemode" | "gm" => {
            if !require_op(player, op) {
                return true;
            }
            let gm = match args.first().map(String::as_str) {
                Some("0") | Some("survival") => 0,
                Some("1") | Some("creative") => 1,
                Some("2") | Some("adventure") => 2,
                Some("3") | Some("spectator") => 3,
                _ => {
                    tell(player, "usage: /gamemode <0-3>", "red");
                    return true;
                }
            };
            set_gamemode(player, gm);
            tell(player, format!("Gamemode set to {gm}"), "green");
        }
        "give" => {
            if !require_op(player, op) {
                return true;
            }
            match args.first().and_then(|n| item::id_any(n)) {
                Some(id) => {
                    let count: u8 = args.get(1).and_then(|c| c.parse().ok()).unwrap_or(1);
                    player.give(id, count);
                    tell(player, format!("Gave {count} x {}", args[0]), "green");
                }
                None => tell(player, "usage: /give <item> [count]", "red"),
            }
        }
        "time" => {
            if !require_op(player, op) {
                return true;
            }
            let t = match args.first().map(String::as_str) {
                Some("day") => 1000,
                Some("noon") => 6000,
                Some("night") => 13000,
                Some("midnight") => 18000,
                Some(n) => n.parse().unwrap_or(1000),
                None => 1000,
            };
            shared.set_time(t);
            shared.broadcast(cb::update_time(0, t));
            tell(player, format!("Time set to {t}"), "green");
        }
        "weather" => {
            if !require_op(player, op) {
                return true;
            }
            match args.first().map(String::as_str) {
                Some("rain") | Some("storm") => {
                    shared.broadcast(cb::game_event(2, 0.0));
                    tell(player, "Weather: rain", "green");
                }
                _ => {
                    shared.broadcast(cb::game_event(1, 0.0));
                    tell(player, "Weather: clear", "green");
                }
            }
        }
        "summon" => {
            if !require_op(player, op) {
                return true;
            }
            match args.first().and_then(|n| MobKind::from_name(n)) {
                Some(kind) => {
                    let s = player.state();
                    let count = args.get(1).and_then(|c| c.parse().ok()).unwrap_or(1u32).min(20);
                    for _ in 0..count {
                        mobs::summon(shared, kind, s.x, s.y, s.z);
                    }
                    tell(player, format!("Summoned {count} {}", kind.name()), "green");
                }
                None => tell(player, "usage: /summon <mob> [count]", "red"),
            }
        }
        "effect" => {
            if !require_op(player, op) {
                return true;
            }
            match args.first().and_then(|n| effect_id(n)) {
                Some(id) => {
                    let secs: i32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(30);
                    let amp: i8 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    player.send(cb::entity_effect(player.entity_id, id, amp, secs * 20, 0x02));
                    tell(player, format!("Applied effect {}", args[0]), "green");
                }
                None => tell(player, "usage: /effect <speed|regeneration|strength|jump_boost|night_vision> [secs] [amp]", "red"),
            }
        }
        "heal" => {
            if !require_op(player, op) {
                return true;
            }
            combat::revive(player);
            tell(player, "Healed.", "green");
        }
        "vehicle" => {
            if !require_op(player, op) {
                return true;
            }
            let (type_id, label) = match args.first().map(String::as_str) {
                Some("minecart") => (64, "minecart"),
                _ => (9, "boat"),
            };
            let s = player.state();
            let entity_id = shared.next_entity_id();
            let uuid = uuid::Uuid::new_v4();
            shared.add_vehicle(Vehicle {
                entity_id,
                type_id,
                uuid,
                x: s.x,
                y: s.y,
                z: s.z,
                yaw: s.yaw,
                rider: None,
            });
            shared.broadcast(cb::spawn_entity(entity_id, uuid, type_id, s.x, s.y, s.z, s.yaw, 0.0, s.yaw, 0, (0, 0, 0)));
            tell(player, format!("Spawned a {label} — right-click to ride, jump to exit."), "green");
        }
        "kill" => {
            combat::damage_player(shared, player, 1000.0, "died");
        }
        "clear" => {
            if !require_op(player, op) {
                return true;
            }
            player.inventory(|inv| *inv = Inventory::default());
            player.sync_inventory();
            tell(player, "Inventory cleared.", "green");
        }
        "xp" => {
            let amount: i32 = args.first().and_then(|a| a.parse().ok()).unwrap_or(10);
            combat::grant_xp(player, amount);
            tell(player, format!("Granted {amount} XP"), "green");
        }
        _ => return false,
    }
    true
}

fn require_op(player: &Player, op: bool) -> bool {
    if !op {
        tell(player, "You don't have permission for that.", "red");
    }
    op
}

/// Change a player's gamemode and update their abilities.
pub fn set_gamemode(player: &Player, gm: i32) {
    player.update(|s| s.gamemode = gm);
    player.send(cb::game_event(3, gm as f32));
    let abilities = if gm == 1 { 0x0D } else { 0x00 };
    player.send(cb::player_abilities(abilities, 0.05, 0.1));
}

fn teleport(shared: &Arc<Shared>, player: &Player, args: &[String]) {
    // /tp <x> <y> <z>
    if let [x, y, z] = args {
        if let (Ok(x), Ok(y), Ok(z)) = (x.parse(), y.parse(), z.parse()) {
            do_teleport(shared, player, x, y, z);
            return;
        }
    }
    // /tp <player>
    if let [target] = args {
        if let Some(t) = shared.player_by_name(target) {
            let s = t.state();
            do_teleport(shared, player, s.x, s.y, s.z);
            return;
        }
    }
    tell(player, "usage: /tp <x> <y> <z> | /tp <player>", "red");
}

fn do_teleport(shared: &Arc<Shared>, player: &Player, x: f64, y: f64, z: f64) {
    player.update(|s| {
        s.x = x;
        s.y = y;
        s.z = z;
        s.fall_peak_y = y;
    });
    player.send(cb::sync_position(x, y, z, 0.0, 0.0, 0, 0));
    let s = player.state();
    shared.broadcast_except(
        player.entity_id,
        cb::entity_teleport(player.entity_id, s.x, s.y, s.z, s.yaw, s.pitch, true),
    );
}

/// Map a status-effect name to its 1.20.1 registry id.
fn effect_id(name: &str) -> Option<i32> {
    Some(match name {
        "speed" => 1,
        "slowness" => 2,
        "haste" => 3,
        "strength" => 5,
        "jump_boost" => 8,
        "regeneration" => 10,
        "resistance" => 11,
        "fire_resistance" => 12,
        "water_breathing" => 13,
        "invisibility" => 14,
        "night_vision" => 16,
        _ => return None,
    })
}
