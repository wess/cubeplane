//! Biome-aware natural spawning: vanilla-style mob categories, per-category
//! population caps and per-biome weighted spawn lists. The mob tick
//! ([`crate::mobs`]) picks a category, looks up the biome at the candidate
//! point and draws a weighted entry from that biome's list for the category.
//!
//! Lists are curated so only environment-appropriate mobs appear: no Nether
//! mobs or bosses spawn naturally, water mobs only in water, and cold biomes
//! swap in their cold variants (husk→desert, stray→snow).

use cubeplane_world::biome;

/// The vanilla spawn groups, each with its own population cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Hostile monsters (spawn in the dark / at night).
    Monster,
    /// Passive land animals.
    Creature,
    /// Fish, squid and dolphins (spawn in water).
    WaterCreature,
    /// Small decorative water life.
    WaterAmbient,
    /// Bats and the like.
    Ambient,
}

/// The category a mob naturally belongs to (used to enforce per-group caps).
pub fn category_of(name: &str) -> Category {
    match name {
        "cod" | "salmon" | "squid" | "glow_squid" | "dolphin" => Category::WaterCreature,
        "tropical_fish" | "pufferfish" => Category::WaterAmbient,
        "bat" => Category::Ambient,
        _ => {
            if is_passive(name) {
                Category::Creature
            } else {
                Category::Monster
            }
        }
    }
}

fn is_passive(name: &str) -> bool {
    crate::entity::MobKind::from_name(name).map(|k| !k.hostile()).unwrap_or(false)
}

/// The population cap for a category, scaled by the online player count.
pub fn cap(category: Category, players: usize) -> usize {
    let p = players.max(1);
    match category {
        Category::Monster => (p * 12).min(60),
        Category::Creature => (p * 4).min(20),
        Category::WaterCreature => (p * 3).min(15),
        Category::WaterAmbient => (p * 2).min(10),
        Category::Ambient => (p * 2).min(8),
    }
}

/// The weighted `(mob, weight)` spawn entries for a biome and category. An empty
/// slice means nothing of that category spawns there (e.g. land animals in the
/// ocean), and the caller should pick a different category or skip.
pub fn list(biome_id: u16, category: Category) -> &'static [(&'static str, u32)] {
    match category {
        Category::Monster => monsters(biome_id),
        Category::Creature => creatures(biome_id),
        Category::WaterCreature => water_creatures(biome_id),
        Category::WaterAmbient => &[("tropical_fish", 10), ("pufferfish", 2)],
        Category::Ambient => &[("bat", 10)],
    }
}

fn monsters(biome_id: u16) -> &'static [(&'static str, u32)] {
    match biome_id {
        biome::DESERT => {
            &[("husk", 95), ("skeleton", 100), ("creeper", 100), ("spider", 100), ("enderman", 10), ("slime", 60)]
        }
        biome::SNOWY_PLAINS | biome::SNOWY_TAIGA => {
            &[("zombie", 95), ("stray", 80), ("skeleton", 20), ("creeper", 100), ("spider", 100), ("enderman", 10)]
        }
        biome::SWAMP => {
            &[("zombie", 80), ("skeleton", 100), ("creeper", 100), ("spider", 100), ("slime", 100), ("witch", 10), ("enderman", 10)]
        }
        _ => {
            &[("zombie", 95), ("skeleton", 100), ("creeper", 100), ("spider", 100), ("enderman", 10), ("witch", 5), ("slime", 50)]
        }
    }
}

fn creatures(biome_id: u16) -> &'static [(&'static str, u32)] {
    match biome_id {
        biome::PLAINS => &[("cow", 8), ("sheep", 12), ("pig", 10), ("chicken", 10), ("horse", 5), ("donkey", 1)],
        biome::FOREST | biome::BIRCH_FOREST => &[("cow", 8), ("sheep", 8), ("pig", 10), ("chicken", 10), ("wolf", 5)],
        biome::TAIGA => &[("wolf", 8), ("rabbit", 4), ("fox", 8), ("sheep", 6), ("pig", 6), ("chicken", 6), ("cow", 6)],
        biome::SNOWY_PLAINS => &[("rabbit", 10), ("polar_bear", 1)],
        biome::SNOWY_TAIGA => &[("rabbit", 10), ("wolf", 8), ("fox", 8)],
        biome::DESERT => &[("rabbit", 4)],
        biome::SAVANNA => &[("horse", 1), ("donkey", 1), ("cow", 8), ("sheep", 2), ("llama", 4)],
        biome::JUNGLE => &[("parrot", 10), ("ocelot", 2), ("panda", 1), ("chicken", 10), ("pig", 10)],
        biome::SWAMP => &[("frog", 10), ("chicken", 10)],
        biome::BEACH => &[("turtle", 5)],
        biome::WINDSWEPT_HILLS => &[("llama", 5), ("goat", 10)],
        // Oceans/rivers have no land animals.
        biome::OCEAN | biome::RIVER => &[],
        _ => &[("cow", 8), ("sheep", 12), ("pig", 10), ("chicken", 10)],
    }
}

fn water_creatures(biome_id: u16) -> &'static [(&'static str, u32)] {
    match biome_id {
        biome::OCEAN => &[("cod", 10), ("salmon", 5), ("squid", 4), ("dolphin", 2)],
        biome::RIVER => &[("salmon", 5), ("squid", 2)],
        _ => &[("squid", 4)],
    }
}

/// Draw a weighted mob name from a list, or `None` if the list is empty.
pub fn pick<'a>(list: &'a [(&'a str, u32)], rng: &mut impl rand::Rng) -> Option<&'a str> {
    let total: u32 = list.iter().map(|(_, w)| *w).sum();
    if total == 0 {
        return None;
    }
    let mut roll = rng.gen_range(0..total);
    for (name, weight) in list {
        if roll < *weight {
            return Some(name);
        }
        roll -= *weight;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_match_intent() {
        assert_eq!(category_of("zombie"), Category::Monster);
        assert_eq!(category_of("cow"), Category::Creature);
        assert_eq!(category_of("cod"), Category::WaterCreature);
        assert_eq!(category_of("bat"), Category::Ambient);
        assert_eq!(category_of("pufferfish"), Category::WaterAmbient);
    }

    #[test]
    fn no_nether_mobs_or_bosses_spawn_naturally() {
        // Sweep every biome × category; the union must exclude Nether mobs and bosses.
        let banned = ["blaze", "ghast", "magma_cube", "piglin", "hoglin", "strider", "zombified_piglin", "wither_skeleton", "ender_dragon", "wither", "warden", "elder_guardian"];
        for b in biome::BIOMES {
            for cat in [Category::Monster, Category::Creature, Category::WaterCreature, Category::WaterAmbient, Category::Ambient] {
                for (name, _) in list(b.id, cat) {
                    assert!(!banned.contains(name), "{name} should not spawn naturally (biome {})", b.name);
                    assert!(crate::entity::MobKind::from_name(name).is_some(), "unknown mob {name}");
                }
            }
        }
    }

    #[test]
    fn land_animals_never_listed_for_open_water() {
        assert!(list(biome::OCEAN, Category::Creature).is_empty());
        assert!(!list(biome::OCEAN, Category::WaterCreature).is_empty());
    }

    #[test]
    fn desert_uses_husk_not_zombie() {
        let names: Vec<_> = list(biome::DESERT, Category::Monster).iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"husk"));
        assert!(!names.contains(&"zombie"));
    }
}
