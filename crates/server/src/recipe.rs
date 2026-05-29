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
fn raw_recipes() -> &'static [RawRecipe] {
    &[
        ("oak_planks", 4, &[("oak_log", 1)]),
        ("stick", 4, &[("oak_planks", 2)]),
        ("crafting_table", 1, &[("oak_planks", 4)]),
        ("chest", 1, &[("oak_planks", 8)]),
        ("furnace", 1, &[("cobblestone", 8)]),
        ("torch", 4, &[("coal", 1), ("stick", 1)]),
        ("oak_door", 3, &[("oak_planks", 6)]),
        ("ladder", 3, &[("stick", 7)]),
        ("bread", 1, &[("wheat", 3)]),
        ("bowl", 4, &[("oak_planks", 3)]),
        ("wooden_sword", 1, &[("oak_planks", 2), ("stick", 1)]),
        ("wooden_pickaxe", 1, &[("oak_planks", 3), ("stick", 2)]),
        ("stone_sword", 1, &[("cobblestone", 2), ("stick", 1)]),
        ("stone_pickaxe", 1, &[("cobblestone", 3), ("stick", 2)]),
        ("iron_sword", 1, &[("iron_ingot", 2), ("stick", 1)]),
        ("iron_pickaxe", 1, &[("iron_ingot", 3), ("stick", 2)]),
        ("iron_helmet", 1, &[("iron_ingot", 5)]),
        ("iron_chestplate", 1, &[("iron_ingot", 8)]),
        ("iron_leggings", 1, &[("iron_ingot", 7)]),
        ("iron_boots", 1, &[("iron_ingot", 4)]),
        ("shield", 1, &[("oak_planks", 6), ("iron_ingot", 1)]),
        ("bow", 1, &[("stick", 3), ("string", 3)]),
        ("glass", 1, &[("sand", 1)]), // also smeltable; handy as a craft too
    ]
}

/// Smelting recipes by name: `input -> output`.
fn raw_smelting() -> &'static [(&'static str, &'static str)] {
    &[
        ("iron_ore", "iron_ingot"),
        ("gold_ore", "gold_ingot"),
        ("sand", "glass"),
        ("cobblestone", "stone"),
        ("porkchop", "cooked_porkchop"),
        ("beef", "cooked_beef"),
        ("mutton", "cooked_mutton"),
        ("potato", "baked_potato"),
        ("kelp", "dried_kelp"),
        ("oak_log", "charcoal"),
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
