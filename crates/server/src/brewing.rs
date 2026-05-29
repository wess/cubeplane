//! Brewing-stand block entities: an ingredient brews water/base bottles into
//! potions over time. Simplified (no awkward-base or fuel requirement): the
//! ingredient directly determines the resulting potion.

use std::sync::Arc;

use crate::clientbound as cb;
use crate::item::{self, ItemStack};
use crate::player::Player;
use crate::state::Shared;

/// Window id for brewing-stand screens.
pub const WINDOW: u8 = 5;
/// Ticks to brew one batch.
const BREW_TIME: u32 = 400;

fn window_items(shared: &Arc<Shared>, pos: (i32, i32, i32), player: &Player) -> Vec<ItemStack> {
    let (b, ing) = shared.with_brewing(pos, |s| (s.bottles, s.ingredient)).unwrap_or_default();
    let inv = player.inventory(|i| i.slots().to_vec());
    // Slots: 0-2 bottles, 3 ingredient, 4 blaze powder (unused), then inventory.
    let mut v = vec![b[0], b[1], b[2], ing, ItemStack::EMPTY];
    v.extend_from_slice(&inv[9..45]);
    v
}

/// Open the brewing-stand screen for `player`.
pub fn open(shared: &Arc<Shared>, player: &Player, pos: (i32, i32, i32)) {
    shared.ensure_brewing(pos);
    player.update(|s| s.open_brewing = Some(pos));
    player.send(cb::open_window(WINDOW as i32, 10, &crate::text::plain("Brewing Stand"))); // 10 = brewing menu
    let items = window_items(shared, pos, player);
    player.send(cb::window_items(WINDOW, 0, &items, ItemStack::EMPTY));
}

/// Apply a click to the open brewing stand.
pub fn click(shared: &Arc<Shared>, player: &Player, pos: (i32, i32, i32), changed: &[(i16, ItemStack)]) {
    for (slot, stack) in changed {
        if *slot < 0 {
            continue;
        }
        match *slot {
            0..=2 => {
                shared.with_brewing(pos, |s| s.bottles[*slot as usize] = *stack);
            }
            3 => {
                shared.with_brewing(pos, |s| s.ingredient = *stack);
            }
            4 => {} // fuel slot ignored
            s => {
                let inv_slot = s as usize - 5 + 9;
                player.inventory(|i| i.set(inv_slot, *stack));
            }
        }
    }
}

/// Advance all brewing stands and push updates to viewers.
pub fn tick(shared: &Arc<Shared>) {
    let positions = shared.brewing_positions();
    if positions.is_empty() {
        return;
    }
    let players = shared.players();
    let potion_item = item::id_any("potion");
    for pos in positions {
        let snapshot = shared.with_brewing(pos, |s| {
            step(s, potion_item);
            (s.bottles, s.ingredient, s.brew_time)
        });
        let Some((bottles, ingredient, brew_time)) = snapshot else {
            continue;
        };
        for p in players.iter().filter(|p| p.state().open_brewing == Some(pos)) {
            p.send(cb::set_slot(WINDOW as i8, 0, 0, bottles[0]));
            p.send(cb::set_slot(WINDOW as i8, 0, 1, bottles[1]));
            p.send(cb::set_slot(WINDOW as i8, 0, 2, bottles[2]));
            p.send(cb::set_slot(WINDOW as i8, 0, 3, ingredient));
            p.send(cb::window_property(WINDOW, 0, (BREW_TIME - brew_time) as i16)); // brew time left
        }
    }
}

/// One brewing step.
fn step(s: &mut crate::state::Brewing, potion_item: Option<i32>) {
    let Some(result) = item::brew_result(s.ingredient.id) else {
        s.brew_time = 0;
        return;
    };
    let any_bottle = s.bottles.iter().any(|b| potion_item.is_some() && b.id == potion_item.unwrap());
    if !any_bottle {
        s.brew_time = 0;
        return;
    }
    s.brew_time += 1;
    if s.brew_time >= BREW_TIME {
        for b in s.bottles.iter_mut() {
            if Some(b.id) == potion_item {
                b.potion = result;
            }
        }
        s.ingredient.count -= 1;
        if s.ingredient.count == 0 {
            s.ingredient = ItemStack::EMPTY;
        }
        s.brew_time = 0;
    }
}
