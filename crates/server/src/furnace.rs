//! Furnace block entities: a 3-slot (input/fuel/output) smelter that progresses
//! over time, consuming fuel and turning smeltable inputs into outputs.

use std::sync::Arc;

use crate::clientbound as cb;
use crate::item::ItemStack;
use crate::player::Player;
use crate::recipe;
use crate::state::Shared;

/// The window id used for furnace screens.
pub const WINDOW: u8 = 4;
/// Total ticks to smelt one item.
const COOK_TIME: u32 = 200;

/// Combined furnace + player inventory contents for the window.
fn window_items(shared: &Arc<Shared>, pos: (i32, i32, i32), player: &Player) -> Vec<ItemStack> {
    let (input, fuel, output) = shared
        .with_furnace(pos, |f| (f.input, f.fuel, f.output))
        .unwrap_or_default();
    let inv = player.inventory(|i| i.slots().to_vec());
    let mut v = vec![input, fuel, output];
    v.extend_from_slice(&inv[9..45]); // main + hotbar
    v
}

/// Open the furnace screen for `player`.
pub fn open(shared: &Arc<Shared>, player: &Player, pos: (i32, i32, i32)) {
    shared.ensure_furnace(pos);
    player.update(|s| s.open_furnace = Some(pos));
    player.send(cb::open_window(WINDOW as i32, 13, &crate::text::plain("Furnace"))); // 13 = furnace menu
    let items = window_items(shared, pos, player);
    player.send(cb::window_items(WINDOW, 0, &items, ItemStack::EMPTY));
}

/// Apply a window click to the open furnace: slots 0/1/2 are furnace slots, the
/// rest map to the player inventory.
pub fn click(shared: &Arc<Shared>, player: &Player, pos: (i32, i32, i32), changed: &[(i16, ItemStack)]) {
    for (slot, stack) in changed {
        if *slot < 0 {
            continue;
        }
        match *slot {
            0 => {
                shared.with_furnace(pos, |f| f.input = *stack);
            }
            1 => {
                shared.with_furnace(pos, |f| f.fuel = *stack);
            }
            2 => {
                shared.with_furnace(pos, |f| f.output = *stack);
            }
            s => {
                let inv_slot = s as usize - 3 + 9;
                player.inventory(|i| i.set(inv_slot, *stack));
            }
        }
    }
}

/// Advance all furnaces one step and push slot/progress updates to viewers.
pub fn tick(shared: &Arc<Shared>) {
    let positions = shared.furnace_positions();
    if positions.is_empty() {
        return;
    }
    let players = shared.players();
    for pos in positions {
        let snapshot = shared.with_furnace(pos, |f| {
            step(f);
            (f.input, f.fuel, f.output, f.burn, f.burn_total, f.cook)
        });
        let Some((input, fuel, output, burn, burn_total, cook)) = snapshot else {
            continue;
        };
        // Update anyone watching this furnace.
        for p in players.iter().filter(|p| p.state().open_furnace == Some(pos)) {
            p.send(cb::set_slot(WINDOW as i8, 0, 0, input));
            p.send(cb::set_slot(WINDOW as i8, 0, 1, fuel));
            p.send(cb::set_slot(WINDOW as i8, 0, 2, output));
            p.send(cb::window_property(WINDOW, 0, burn as i16)); // fuel left
            p.send(cb::window_property(WINDOW, 1, burn_total.max(1) as i16)); // fuel total
            p.send(cb::window_property(WINDOW, 2, cook as i16)); // cook progress
            p.send(cb::window_property(WINDOW, 3, COOK_TIME as i16)); // cook total
        }
    }
}

/// One smelting step for a single furnace.
fn step(f: &mut crate::state::Furnace) {
    let can_smelt = !f.input.is_empty()
        && recipe::smelt(f.input.id).is_some()
        && {
            let result = recipe::smelt(f.input.id).unwrap();
            f.output.is_empty() || (f.output.id == result && f.output.count < 64)
        };

    // Light the furnace from fuel if needed.
    if f.burn == 0 && can_smelt && !f.fuel.is_empty() {
        let t = recipe::fuel_ticks(f.fuel.id);
        if t > 0 {
            f.burn = t;
            f.burn_total = t;
            f.fuel.count -= 1;
            if f.fuel.count == 0 {
                f.fuel = ItemStack::EMPTY;
            }
        }
    }

    if f.burn > 0 {
        f.burn -= 1;
        if can_smelt {
            f.cook += 1;
            if f.cook >= COOK_TIME {
                let result = recipe::smelt(f.input.id).unwrap();
                if f.output.is_empty() {
                    f.output = ItemStack::new(result, 1);
                } else {
                    f.output.count += 1;
                }
                f.input.count -= 1;
                if f.input.count == 0 {
                    f.input = ItemStack::EMPTY;
                }
                f.cook = 0;
            }
        } else {
            f.cook = f.cook.saturating_sub(2);
        }
    } else {
        f.cook = f.cook.saturating_sub(2);
    }
}
