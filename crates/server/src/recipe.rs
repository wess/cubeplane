//! Crafting and smelting recipes.
//!
//! Recipes are declared by item *name* and resolved against the full item
//! registry, so any vanilla item can be an ingredient or result. Crafting is
//! modelled as an ingredient multiset (which also drives the recipe-book style
//! `/craft` command and the crafting-table grid), and smelting as a simple
//! input → output map with a fuel table.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::item;

/// A crafting recipe: a result plus the ingredients it consumes.
#[derive(Debug, Clone)]
pub struct Recipe {
    pub output_id: i32,
    pub output_count: u8,
    /// `(ingredient id, count)` consumed.
    pub ingredients: Vec<(i32, u8)>,
}

/// A declarative recipe row: `(output, count, &[(ingredient, count)])`.
type RawRecipe = (&'static str, u8, &'static [(&'static str, u8)]);

/// Declarative recipe table by item name. `out, count <= [(name, count), …]`.
/// Unknown item names are dropped at load, so this can list liberally.
fn raw_recipes() -> &'static [RawRecipe] {
    &[
        // Wood → planks (every common species).
        ("oak_planks", 4, &[("oak_log", 1)]),
        ("spruce_planks", 4, &[("spruce_log", 1)]),
        ("birch_planks", 4, &[("birch_log", 1)]),
        ("jungle_planks", 4, &[("jungle_log", 1)]),
        ("acacia_planks", 4, &[("acacia_log", 1)]),
        ("dark_oak_planks", 4, &[("dark_oak_log", 1)]),
        // Core blocks & utilities.
        ("stick", 4, &[("oak_planks", 2)]),
        ("crafting_table", 1, &[("oak_planks", 4)]),
        ("chest", 1, &[("oak_planks", 8)]),
        ("furnace", 1, &[("cobblestone", 8)]),
        ("brewing_stand", 1, &[("cobblestone", 3), ("blaze_rod", 1)]),
        ("anvil", 1, &[("iron_block", 3), ("iron_ingot", 4)]),
        ("torch", 4, &[("coal", 1), ("stick", 1)]),
        ("oak_door", 3, &[("oak_planks", 6)]),
        ("oak_trapdoor", 2, &[("oak_planks", 6)]),
        ("oak_fence", 3, &[("oak_planks", 4), ("stick", 2)]),
        ("oak_slab", 6, &[("oak_planks", 3)]),
        ("oak_stairs", 4, &[("oak_planks", 6)]),
        ("ladder", 3, &[("stick", 7)]),
        ("bowl", 4, &[("oak_planks", 3)]),
        ("bookshelf", 1, &[("oak_planks", 6), ("book", 3)]),
        ("bed", 1, &[("white_wool", 3), ("oak_planks", 3)]),
        ("glass", 1, &[("sand", 1)]),
        ("glowstone", 1, &[("glowstone_dust", 4)]),
        ("iron_block", 1, &[("iron_ingot", 9)]),
        ("gold_block", 1, &[("gold_ingot", 9)]),
        ("diamond_block", 1, &[("diamond", 9)]),
        ("tnt", 1, &[("gunpowder", 5), ("sand", 4)]),
        ("book", 1, &[("paper", 3), ("leather", 1)]),
        // Food.
        ("bread", 1, &[("wheat", 3)]),
        ("golden_apple", 1, &[("apple", 1), ("gold_ingot", 8)]),
        ("golden_carrot", 1, &[("carrot", 1), ("gold_nugget", 8)]),
        // Combat / misc tools.
        ("bow", 1, &[("stick", 3), ("string", 3)]),
        ("arrow", 4, &[("flint", 1), ("stick", 1), ("feather", 1)]),
        ("shield", 1, &[("oak_planks", 6), ("iron_ingot", 1)]),
        ("shears", 1, &[("iron_ingot", 2)]),
        ("bucket", 1, &[("iron_ingot", 3)]),
        ("flint_and_steel", 1, &[("iron_ingot", 1), ("flint", 1)]),
        ("fishing_rod", 1, &[("stick", 3), ("string", 2)]),
        // Tools & weapons, all tiers (sword/pickaxe/axe/shovel/hoe).
        ("wooden_sword", 1, &[("oak_planks", 2), ("stick", 1)]),
        ("wooden_pickaxe", 1, &[("oak_planks", 3), ("stick", 2)]),
        ("wooden_axe", 1, &[("oak_planks", 3), ("stick", 2)]),
        ("wooden_shovel", 1, &[("oak_planks", 1), ("stick", 2)]),
        ("wooden_hoe", 1, &[("oak_planks", 2), ("stick", 2)]),
        ("stone_sword", 1, &[("cobblestone", 2), ("stick", 1)]),
        ("stone_pickaxe", 1, &[("cobblestone", 3), ("stick", 2)]),
        ("stone_axe", 1, &[("cobblestone", 3), ("stick", 2)]),
        ("stone_shovel", 1, &[("cobblestone", 1), ("stick", 2)]),
        ("stone_hoe", 1, &[("cobblestone", 2), ("stick", 2)]),
        ("iron_sword", 1, &[("iron_ingot", 2), ("stick", 1)]),
        ("iron_pickaxe", 1, &[("iron_ingot", 3), ("stick", 2)]),
        ("iron_axe", 1, &[("iron_ingot", 3), ("stick", 2)]),
        ("iron_shovel", 1, &[("iron_ingot", 1), ("stick", 2)]),
        ("iron_hoe", 1, &[("iron_ingot", 2), ("stick", 2)]),
        ("golden_sword", 1, &[("gold_ingot", 2), ("stick", 1)]),
        ("golden_pickaxe", 1, &[("gold_ingot", 3), ("stick", 2)]),
        ("diamond_sword", 1, &[("diamond", 2), ("stick", 1)]),
        ("diamond_pickaxe", 1, &[("diamond", 3), ("stick", 2)]),
        ("diamond_axe", 1, &[("diamond", 3), ("stick", 2)]),
        ("diamond_shovel", 1, &[("diamond", 1), ("stick", 2)]),
        ("diamond_hoe", 1, &[("diamond", 2), ("stick", 2)]),
        // Armour: iron, gold, diamond, leather (helmet/chest/legs/boots).
        ("iron_helmet", 1, &[("iron_ingot", 5)]),
        ("iron_chestplate", 1, &[("iron_ingot", 8)]),
        ("iron_leggings", 1, &[("iron_ingot", 7)]),
        ("iron_boots", 1, &[("iron_ingot", 4)]),
        ("golden_helmet", 1, &[("gold_ingot", 5)]),
        ("golden_chestplate", 1, &[("gold_ingot", 8)]),
        ("diamond_helmet", 1, &[("diamond", 5)]),
        ("diamond_chestplate", 1, &[("diamond", 8)]),
        ("diamond_leggings", 1, &[("diamond", 7)]),
        ("diamond_boots", 1, &[("diamond", 4)]),
        ("leather_helmet", 1, &[("leather", 5)]),
        ("leather_chestplate", 1, &[("leather", 8)]),
        ("leather_leggings", 1, &[("leather", 7)]),
        ("leather_boots", 1, &[("leather", 4)]),
    ]
}

/// Smelting recipes by name: `input -> output`.
fn raw_smelting() -> &'static [(&'static str, &'static str)] {
    &[
        ("iron_ore", "iron_ingot"),
        ("deepslate_iron_ore", "iron_ingot"),
        ("raw_iron", "iron_ingot"),
        ("gold_ore", "gold_ingot"),
        ("deepslate_gold_ore", "gold_ingot"),
        ("raw_gold", "gold_ingot"),
        ("copper_ore", "copper_ingot"),
        ("raw_copper", "copper_ingot"),
        ("ancient_debris", "netherite_scrap"),
        ("sand", "glass"),
        ("cobblestone", "stone"),
        ("stone", "smooth_stone"),
        ("clay_ball", "brick"),
        ("clay", "terracotta"),
        ("netherrack", "netherrack"),
        ("cactus", "green_dye"),
        ("porkchop", "cooked_porkchop"),
        ("beef", "cooked_beef"),
        ("chicken", "cooked_chicken"),
        ("mutton", "cooked_mutton"),
        ("rabbit", "cooked_rabbit"),
        ("cod", "cooked_cod"),
        ("salmon", "cooked_salmon"),
        ("potato", "baked_potato"),
        ("kelp", "dried_kelp"),
        ("oak_log", "charcoal"),
        ("spruce_log", "charcoal"),
        ("birch_log", "charcoal"),
    ]
}

fn crafting_table() -> &'static Vec<Recipe> {
    static TABLE: OnceLock<Vec<Recipe>> = OnceLock::new();
    TABLE.get_or_init(|| {
        raw_recipes()
            .iter()
            .filter_map(|(out, count, ings)| {
                let output_id = item::id_any(out)?;
                let ingredients: Vec<(i32, u8)> =
                    ings.iter().filter_map(|(n, c)| item::id_any(n).map(|id| (id, *c))).collect();
                if ingredients.len() != ings.len() {
                    return None;
                }
                Some(Recipe { output_id, output_count: *count, ingredients })
            })
            .collect()
    })
}

fn smelting_table() -> &'static HashMap<i32, i32> {
    static TABLE: OnceLock<HashMap<i32, i32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        raw_smelting()
            .iter()
            .filter_map(|(i, o)| Some((item::id_any(i)?, item::id_any(o)?)))
            .collect()
    })
}

/// Find a crafting recipe that produces `output_id`.
pub fn recipe_for(output_id: i32) -> Option<&'static Recipe> {
    crafting_table().iter().find(|r| r.output_id == output_id)
}

/// Look up a craftable output id by name.
pub fn craftable(name: &str) -> Option<&'static Recipe> {
    let id = item::id_any(name)?;
    recipe_for(id)
}

/// All craftable output names (for help / the recipe book).
pub fn craftable_names() -> Vec<&'static str> {
    raw_recipes().iter().map(|(n, _, _)| *n).collect()
}

/// The smelting result for an input item, if any.
pub fn smelt(input_id: i32) -> Option<i32> {
    smelting_table().get(&input_id).copied()
}

/// Burn time in ticks for a fuel item (0 if not a fuel).
pub fn fuel_ticks(item_id: i32) -> u32 {
    let name = item::name_of(item_id).unwrap_or("");
    match name {
        "coal" | "charcoal" => 1600,
        "lava_bucket" => 20000,
        "blaze_rod" => 2400,
        n if n.ends_with("_log") || n.ends_with("_planks") => 300,
        "stick" => 100,
        _ => 0,
    }
}

/// Match a crafting grid (slot item ids, `None` = empty) to a recipe, ignoring
/// layout (shapeless-style): the grid's non-empty items must exactly satisfy a
/// recipe's ingredient multiset.
pub fn match_grid(grid: &[Option<i32>]) -> Option<&'static Recipe> {
    let mut have: HashMap<i32, u8> = HashMap::new();
    for id in grid.iter().flatten() {
        *have.entry(*id).or_insert(0) += 1;
    }
    if have.is_empty() {
        return None;
    }
    crafting_table().iter().find(|r| {
        let mut need: HashMap<i32, u8> = HashMap::new();
        for (id, c) in &r.ingredients {
            *need.entry(*id).or_insert(0) += c;
        }
        need == have
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planks_and_sticks_resolve() {
        let log = item::id_any("oak_log").unwrap();
        let planks = item::id_any("oak_planks").unwrap();
        let r = recipe_for(planks).unwrap();
        assert_eq!(r.ingredients, vec![(log, 1)]);
        assert_eq!(r.output_count, 4);
    }

    #[test]
    fn grid_matches_shapelessly() {
        let planks = item::id_any("oak_planks").unwrap();
        let grid = vec![Some(planks), Some(planks), None, None];
        let r = match_grid(&grid).unwrap();
        assert_eq!(r.output_id, item::id_any("stick").unwrap());
    }

    #[test]
    fn smelting_and_fuel() {
        let iron_ore = item::id_any("iron_ore").unwrap();
        assert_eq!(smelt(iron_ore), item::id_any("iron_ingot"));
        assert_eq!(fuel_ticks(item::id_any("coal").unwrap()), 1600);
        assert_eq!(fuel_ticks(item::id_any("diamond").unwrap()), 0);
    }
}
