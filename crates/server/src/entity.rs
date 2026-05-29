//! Non-player entities (mobs) and their kinds.
//!
//! Entity type ids are the Minecraft 1.20.1 registry ids (from
//! `minecraft-data` `pc/1.20`). Each [`MobKind`] bundles the gameplay tuning
//! cubeplane needs: max health, whether it is hostile, walk speed and the
//! melee damage it deals.

use uuid::Uuid;

/// Entity type id for a dropped item.
pub const ITEM_ENTITY: i32 = 54;
/// Entity type id for an arrow.
pub const ARROW: i32 = 3;
/// Entity type id for primed TNT.
pub const TNT: i32 = 101;

/// A handle to one of the game's living mob types (index into the generated
/// [`crate::mobs_table::MOBS`] table). All ~80 living entities are available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MobKind(usize);

impl MobKind {
    fn row(self) -> &'static crate::mobs_table::MobRow {
        &crate::mobs_table::MOBS[self.0]
    }

    /// The 1.20.1 entity type registry id.
    pub fn type_id(self) -> i32 {
        self.row().type_id
    }

    /// Lowercase identifier (without the `minecraft:` prefix).
    pub fn name(self) -> &'static str {
        self.row().name
    }

    /// Maximum (and spawn) health in half-heart units.
    pub fn max_health(self) -> f32 {
        self.row().max_health
    }

    /// Whether this mob hunts and attacks players.
    pub fn hostile(self) -> bool {
        self.row().hostile
    }

    /// Horizontal movement speed in blocks per tick.
    pub fn speed(self) -> f64 {
        self.row().speed
    }

    /// Melee damage dealt to a player (half-hearts).
    pub fn attack_damage(self) -> f32 {
        self.row().attack
    }

    /// The item names that put this animal into love mode (breeding). Empty for
    /// mobs that can't be bred. Matches Minecraft 1.20.1 feeding rules.
    pub fn breeding_food(self) -> &'static [&'static str] {
        match self.name() {
            "cow" | "mooshroom" | "sheep" | "goat" => &["wheat"],
            "pig" => &["carrot", "potato", "beetroot"],
            "chicken" => &["wheat_seeds", "beetroot_seeds", "melon_seeds", "pumpkin_seeds"],
            "rabbit" => &["carrot", "golden_carrot", "dandelion"],
            "wolf" => &["beef", "porkchop", "chicken", "mutton", "rabbit", "cooked_beef", "cooked_porkchop", "cooked_chicken", "cooked_mutton", "cooked_rabbit"],
            "cat" | "ocelot" => &["cod", "salmon"],
            "horse" | "donkey" => &["golden_apple", "golden_carrot"],
            "llama" | "trader_llama" => &["hay_block"],
            "turtle" => &["seagrass"],
            "panda" => &["bamboo"],
            "fox" => &["sweet_berries", "glow_berries"],
            "bee" => &["dandelion", "poppy", "blue_orchid", "allium", "azure_bluet", "cornflower"],
            "axolotl" => &["tropical_fish_bucket"],
            "frog" => &["slime_ball"],
            "strider" => &["warped_fungus"],
            "hoglin" => &["crimson_fungus"],
            _ => &[],
        }
    }

    /// Whether this animal can be bred by feeding.
    pub fn can_breed(self) -> bool {
        !self.breeding_food().is_empty()
    }

    /// Parse a kind from its lowercase name.
    pub fn from_name(name: &str) -> Option<MobKind> {
        let key = name.strip_prefix("minecraft:").unwrap_or(name);
        crate::mobs_table::MOBS
            .iter()
            .position(|m| m.name == key)
            .map(MobKind)
    }

    /// A random mob kind, optionally restricted to passive animals.
    pub fn random(rng: &mut impl rand::Rng, passive_only: bool) -> MobKind {
        loop {
            let i = rng.gen_range(0..crate::mobs_table::MOBS.len());
            if !passive_only || !crate::mobs_table::MOBS[i].hostile {
                return MobKind(i);
            }
        }
    }
}

/// A dropped item lying in the world, awaiting pickup.
#[derive(Debug, Clone)]
pub struct ItemEntity {
    pub entity_id: i32,
    pub uuid: Uuid,
    pub item_id: i32,
    pub count: u8,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub on_ground: bool,
    /// Ticks alive (despawns after a while).
    pub age: u32,
    /// Ticks before it can be picked up.
    pub pickup_delay: u32,
}

/// A flying projectile (currently only skeleton arrows).
#[derive(Debug, Clone)]
pub struct Projectile {
    pub entity_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub vx: f64,
    pub vy: f64,
    pub vz: f64,
    pub damage: f32,
    pub age: u32,
    /// Entity id of the mob that fired it (so it can't hit its owner).
    pub owner: i32,
}

/// A rideable vehicle (boat or minecart).
#[derive(Debug, Clone)]
pub struct Vehicle {
    pub entity_id: i32,
    pub type_id: i32,
    pub uuid: Uuid,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    /// Entity id of the player currently riding, if any.
    pub rider: Option<i32>,
}

/// A live mob in the world.
#[derive(Debug, Clone)]
pub struct Mob {
    pub entity_id: i32,
    pub uuid: Uuid,
    pub kind: MobKind,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub health: f32,
    pub on_ground: bool,
    /// Current wander heading in radians (when not chasing).
    pub heading: f32,
    /// Cosmetic variant (e.g. sheep wool colour 0-15).
    pub variant: u8,
    /// Whether this is a baby animal.
    pub baby: bool,
    /// Ticks until a baby grows into an adult (0 once grown).
    pub baby_age: u32,
    /// Ticks remaining in "love mode" for breeding (0 = not breeding).
    pub in_love: u32,
    /// Ticks before this animal can breed again.
    pub breed_cooldown: u32,
    /// Ticks until the mob may attack again.
    pub attack_cooldown: u32,
    /// Ticks remaining to play the death animation before removal; `None`
    /// while alive.
    pub dying: Option<u32>,
}

impl Mob {
    pub fn new(entity_id: i32, kind: MobKind, x: f64, y: f64, z: f64, heading: f32) -> Self {
        Mob {
            entity_id,
            uuid: Uuid::new_v4(),
            kind,
            x,
            y,
            z,
            yaw: heading.to_degrees(),
            pitch: 0.0,
            health: kind.max_health(),
            on_ground: false,
            heading,
            variant: 0,
            baby: false,
            baby_age: 0,
            in_love: 0,
            breed_cooldown: 0,
            attack_cooldown: 0,
            dying: None,
        }
    }

    /// Cosmetic metadata for this mob (baby flag, sheep wool colour, …).
    pub fn metadata(&self) -> Vec<crate::clientbound::Meta> {
        let mut m = Vec::new();
        if self.baby {
            m.push(crate::clientbound::Meta::Bool(16, true)); // ageable: is baby
        }
        if self.kind.name() == "sheep" {
            m.push(crate::clientbound::Meta::Byte(17, (self.variant & 0x0f) as i8));
        }
        m
    }

    /// Whether the mob is alive (not in its death animation).
    pub fn alive(&self) -> bool {
        self.dying.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostility_and_lookup() {
        assert!(MobKind::from_name("zombie").unwrap().hostile());
        assert!(!MobKind::from_name("cow").unwrap().hostile());
        assert!(MobKind::from_name("creeper").is_some());
        assert_eq!(MobKind::from_name("not_a_mob"), None);
    }

    #[test]
    fn mob_starts_alive_at_full_health() {
        let pig = MobKind::from_name("pig").unwrap();
        let m = Mob::new(7, pig, 0.0, 64.0, 0.0, 0.0);
        assert!(m.alive());
        assert_eq!(m.health, pig.max_health());
        assert!(!m.baby);
        assert_eq!(m.breed_cooldown, 0);
    }

    #[test]
    fn breeding_food_is_species_specific() {
        let cow = MobKind::from_name("cow").unwrap();
        let pig = MobKind::from_name("pig").unwrap();
        let chicken = MobKind::from_name("chicken").unwrap();
        let zombie = MobKind::from_name("zombie").unwrap();
        assert!(cow.breeding_food().contains(&"wheat"));
        assert!(pig.breeding_food().contains(&"carrot"));
        assert!(!pig.breeding_food().contains(&"wheat"));
        assert!(chicken.breeding_food().contains(&"wheat_seeds"));
        assert!(cow.can_breed());
        // Hostile mobs can't be bred.
        assert!(!zombie.can_breed());
        assert!(zombie.breeding_food().is_empty());
    }
}
