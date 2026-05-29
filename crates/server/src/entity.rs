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
            attack_cooldown: 0,
            dying: None,
        }
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
    }
}
