//! The player inventory: a 46-slot container matching the vanilla window.
//!
//! Slot layout (window id 0):
//! * 0       crafting output
//! * 1–4     crafting grid
//! * 5–8     armor (head, chest, legs, feet)
//! * 9–35    main storage (27)
//! * 36–44   hotbar (9)
//! * 45      offhand
//!
//! cubeplane keeps the model simple — no NBT, no crafting resolution — but it
//! is enough for picking up drops, building from a real stack and wearing armor.

use crate::item::{self, ItemStack};

/// Total slots in the player inventory window.
pub const SIZE: usize = 46;
/// First hotbar slot index.
pub const HOTBAR_START: usize = 36;
/// First armor slot index (head).
pub const ARMOR_START: usize = 5;
/// Main storage range.
const MAIN: std::ops::Range<usize> = 9..36;
const HOTBAR: std::ops::Range<usize> = 36..45;

/// A player's inventory.
#[derive(Debug, Clone)]
pub struct Inventory {
    slots: [ItemStack; SIZE],
}

impl Default for Inventory {
    fn default() -> Self {
        Inventory {
            slots: [ItemStack::EMPTY; SIZE],
        }
    }
}

impl Inventory {
    pub fn get(&self, slot: usize) -> ItemStack {
        self.slots.get(slot).copied().unwrap_or(ItemStack::EMPTY)
    }

    pub fn set(&mut self, slot: usize, stack: ItemStack) {
        if slot < SIZE {
            self.slots[slot] = stack;
        }
    }

    /// The stack in hotbar position `held` (0..9).
    pub fn held(&self, held: u8) -> ItemStack {
        self.get(HOTBAR_START + (held as usize).min(8))
    }

    /// Consume one item from the held hotbar slot; returns the new stack there.
    pub fn consume_held(&mut self, held: u8) -> ItemStack {
        let slot = HOTBAR_START + (held as usize).min(8);
        let mut s = self.get(slot);
        if !s.is_empty() {
            s.count -= 1;
            if s.count == 0 {
                s = ItemStack::EMPTY;
            }
            self.set(slot, s);
        }
        s
    }

    /// All slots, for the Set Container Content packet.
    pub fn slots(&self) -> &[ItemStack] {
        &self.slots
    }

    /// Add an item, merging into existing stacks then filling empty hotbar/main
    /// slots. Returns the leftover count that didn't fit (0 if all stored).
    pub fn add(&mut self, id: i32, mut count: u8) -> u8 {
        let max = item::def(id).map(|d| d.max_stack).unwrap_or(64);
        // First, top up existing stacks of the same item.
        for slot in HOTBAR.chain(MAIN) {
            if count == 0 {
                break;
            }
            let mut s = self.slots[slot];
            if s.id == id && s.count < max {
                let space = max - s.count;
                let moved = space.min(count);
                s.count += moved;
                count -= moved;
                self.slots[slot] = s;
            }
        }
        // Then, fill empty slots.
        for slot in HOTBAR.chain(MAIN) {
            if count == 0 {
                break;
            }
            if self.slots[slot].is_empty() {
                let moved = max.min(count);
                self.slots[slot] = ItemStack::new(id, moved);
                count -= moved;
            }
        }
        count
    }

    /// Total defense points from worn armor.
    pub fn armor_defense(&self) -> f32 {
        (ARMOR_START..ARMOR_START + 4)
            .map(|s| match self.slots[s].def().map(|d| d.kind) {
                Some(crate::item::ItemKind::Armor(_, def)) => def,
                _ => 0.0,
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_merges_and_fills() {
        let mut inv = Inventory::default();
        let stone = item::by_name("stone").unwrap();
        assert_eq!(inv.add(stone, 64), 0);
        assert_eq!(inv.add(stone, 64), 0); // second stack
        // 128 stone across two slots.
        let total: u32 = inv.slots().iter().filter(|s| s.id == stone).map(|s| s.count as u32).sum();
        assert_eq!(total, 128);
    }

    #[test]
    fn consume_held_decrements() {
        let mut inv = Inventory::default();
        inv.set(HOTBAR_START, ItemStack::new(item::by_name("dirt").unwrap(), 3));
        let after = inv.consume_held(0);
        assert_eq!(after.count, 2);
    }
}
