//! Item registry and `ItemStack` model.
//!
//! Item ids are the Minecraft 1.20.1 *item* registry ids (distinct from block
//! state ids), from `minecraft-data` `pc/1.20`. cubeplane ships a curated set
//! covering its placeable blocks plus representative food, weapons, armor and
//! mob drops. Items carry the gameplay stats the engine needs.

use cubeplane_world::block;

/// What an item does when used.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ItemKind {
    /// Placeable block; carries the block-state id to set.
    Block(u16),
    /// Edible; `(hunger restored, saturation)`.
    Food(i32, f32),
    /// Melee weapon dealing this much damage (half-hearts).
    Weapon(f32),
    /// Wearable armor; `(slot 0=head..3=feet, defense points)`.
    Armor(u8, f32),
    /// Everything else (materials, drops, tools).
    Misc,
}

/// Static definition of an item.
#[derive(Debug, Clone, Copy)]
pub struct ItemDef {
    pub id: i32,
    pub name: &'static str,
    pub max_stack: u8,
    pub kind: ItemKind,
}

/// A stack of items. `count == 0` represents an empty slot (encoded as absent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ItemStack {
    pub id: i32,
    pub count: u8,
}

impl ItemStack {
    pub const EMPTY: ItemStack = ItemStack { id: 0, count: 0 };

    pub fn new(id: i32, count: u8) -> Self {
        if id == 0 || count == 0 {
            ItemStack::EMPTY
        } else {
            ItemStack { id, count }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.id == 0 || self.count == 0
    }

    pub fn def(&self) -> Option<&'static ItemDef> {
        def(self.id)
    }
}

macro_rules! items {
    ($($id:expr => $name:literal, $stack:expr, $kind:expr;)+) => {
        const TABLE: &[ItemDef] = &[
            $(ItemDef { id: $id, name: $name, max_stack: $stack, kind: $kind }),+
        ];
    };
}

use ItemKind::*;

items! {
    // Blocks (item id => block-state id)
    1   => "stone", 64, Block(block::STONE);
    14  => "grass_block", 64, Block(block::GRASS_BLOCK);
    15  => "dirt", 64, Block(block::DIRT);
    22  => "cobblestone", 64, Block(block::COBBLESTONE);
    23  => "oak_planks", 64, Block(block::OAK_PLANKS);
    43  => "bedrock", 64, Block(block::BEDROCK);
    44  => "sand", 64, Block(block::SAND);
    48  => "gravel", 64, Block(block::GRAVEL);
    110 => "oak_log", 64, Block(block::OAK_LOG);
    154 => "oak_leaves", 64, Block(block::OAK_LEAVES);
    166 => "glass", 64, Block(block::GLASS);
    277 => "chest", 64, Block(block::CHEST);
    278 => "crafting_table", 64, Block(block::CRAFTING_TABLE);
    // Food (hunger, saturation)
    759 => "apple", 64, Food(4, 2.4);
    815 => "bread", 64, Food(5, 6.0);
    841 => "porkchop", 64, Food(3, 1.8);
    948 => "cooked_beef", 64, Food(8, 12.8);
    951 => "rotten_flesh", 64, Food(4, 0.8);
    1085 => "mutton", 64, Food(2, 1.2);
    // Weapons (damage)
    777 => "wooden_sword", 1, Weapon(4.0);
    782 => "stone_sword", 1, Weapon(5.0);
    792 => "iron_sword", 1, Weapon(6.0);
    // Armor (slot, defense)
    824 => "iron_helmet", 1, Armor(0, 2.0);
    825 => "iron_chestplate", 1, Armor(1, 6.0);
    819 => "leather_boots", 1, Armor(3, 1.0);
    // Materials & mob drops
    269 => "torch", 64, Misc;
    657 => "tnt", 64, Block(block::STONE); // visual placeholder block
    761 => "arrow", 64, Misc;
    764 => "diamond", 64, Misc;
    770 => "iron_ingot", 64, Misc;
    807 => "stick", 64, Misc;
    810 => "string", 64, Misc;
    811 => "feather", 64, Misc;
    812 => "gunpowder", 64, Misc;
    887 => "egg", 16, Misc;
    921 => "bone", 64, Misc;
    959 => "spider_eye", 64, Misc;
}

/// Look up an item definition by id.
pub fn def(id: i32) -> Option<&'static ItemDef> {
    TABLE.iter().find(|d| d.id == id)
}

/// Look up an item id by its (prefix-stripped) name.
pub fn by_name(name: &str) -> Option<i32> {
    let key = name.strip_prefix("minecraft:").unwrap_or(name);
    TABLE.iter().find(|d| d.name == key).map(|d| d.id)
}

/// The item that drops when a block of `state` is broken, if any. Falls back to
/// the full registry: an item whose name matches the block's name.
pub fn item_for_block(state: u16) -> Option<i32> {
    if let Some(id) = TABLE.iter().find_map(|d| match d.kind {
        ItemKind::Block(s) if s == state => Some(d.id),
        _ => None,
    }) {
        return Some(id);
    }
    let name = cubeplane_world::block::info(state).name;
    id_any(name)
}

/// The block state a curated item places, if it is placeable.
pub fn block_for_item(id: i32) -> Option<u16> {
    match def(id)?.kind {
        ItemKind::Block(state) => Some(state),
        _ => None,
    }
}

/// The block state any item places: curated mapping first, otherwise an item
/// whose name matches a block (covers every block item in the game).
pub fn block_state_for_item(id: i32) -> Option<u16> {
    block_for_item(id).or_else(|| cubeplane_world::block::state_by_name(name_of(id)?))
}

/// Maximum stack size for any item (full registry; defaults to 64).
pub fn max_stack(id: i32) -> u8 {
    if let Some(d) = def(id) {
        return d.max_stack;
    }
    crate::items_table::ITEMS
        .binary_search_by(|r| r.id.cmp(&id))
        .ok()
        .map(|i| crate::items_table::ITEMS[i].stack)
        .unwrap_or(64)
}

/// Full item name lookup (every 1.20.1 item).
pub fn name_of(id: i32) -> Option<&'static str> {
    crate::items_table::ITEMS
        .binary_search_by(|r| r.id.cmp(&id))
        .ok()
        .map(|i| crate::items_table::ITEMS[i].name)
}

/// Full item id lookup by name (every 1.20.1 item).
pub fn id_any(name: &str) -> Option<i32> {
    let key = name.strip_prefix("minecraft:").unwrap_or(name);
    crate::items_table::ITEMS.iter().find(|r| r.name == key).map(|r| r.id)
}

/// All curated `(name, id)` pairs, for the mod/command APIs.
#[allow(dead_code)]
pub fn catalog() -> Vec<(&'static str, i32)> {
    TABLE.iter().map(|d| (d.name, d.id)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_roundtrips() {
        let id = by_name("iron_sword").unwrap();
        assert!(matches!(def(id).unwrap().kind, ItemKind::Weapon(_)));
        assert_eq!(block_for_item(by_name("stone").unwrap()), Some(block::STONE));
        assert_eq!(item_for_block(block::DIRT), by_name("dirt"));
    }

    #[test]
    fn empty_stack_normalizes() {
        assert!(ItemStack::new(0, 5).is_empty());
        assert!(ItemStack::new(5, 0).is_empty());
        assert!(!ItemStack::new(1, 1).is_empty());
    }
}
