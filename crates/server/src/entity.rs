//! Non-player entities (mobs) and their kinds.
//!
//! Entity type ids are the Minecraft 1.20.1 registry ids (from
//! `minecraft-data` `pc/1.20`). Each [`MobKind`] bundles the gameplay tuning
//! cubeplane needs: max health, whether it is hostile, walk speed and the
//! melee damage it deals.

use uuid::Uuid;

/// A kind of mob with its registry id and gameplay stats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobKind {
    Zombie,
    Skeleton,
    Spider,
    Creeper,
    Pig,
    Cow,
    Sheep,
    Chicken,
}

impl MobKind {
    /// All kinds, used by the spawner.
    pub const ALL: [MobKind; 8] = [
        MobKind::Zombie,
        MobKind::Skeleton,
        MobKind::Spider,
        MobKind::Creeper,
        MobKind::Pig,
        MobKind::Cow,
        MobKind::Sheep,
        MobKind::Chicken,
    ];

    /// The 1.20.1 entity type registry id.
    pub fn type_id(self) -> i32 {
        match self {
            MobKind::Zombie => 118,
            MobKind::Skeleton => 86,
            MobKind::Spider => 95,
            MobKind::Creeper => 19,
            MobKind::Pig => 72,
            MobKind::Cow => 18,
            MobKind::Sheep => 82,
            MobKind::Chicken => 15,
        }
    }

    /// Lowercase identifier (without the `minecraft:` prefix).
    pub fn name(self) -> &'static str {
        match self {
            MobKind::Zombie => "zombie",
            MobKind::Skeleton => "skeleton",
            MobKind::Spider => "spider",
            MobKind::Creeper => "creeper",
            MobKind::Pig => "pig",
            MobKind::Cow => "cow",
            MobKind::Sheep => "sheep",
            MobKind::Chicken => "chicken",
        }
    }

    /// Maximum (and spawn) health in half-heart units.
    pub fn max_health(self) -> f32 {
        match self {
            MobKind::Zombie => 20.0,
            MobKind::Skeleton => 20.0,
            MobKind::Spider => 16.0,
            MobKind::Creeper => 20.0,
            MobKind::Pig => 10.0,
            MobKind::Cow => 10.0,
            MobKind::Sheep => 8.0,
            MobKind::Chicken => 4.0,
        }
    }

    /// Whether this mob hunts and attacks players.
    pub fn hostile(self) -> bool {
        matches!(
            self,
            MobKind::Zombie | MobKind::Skeleton | MobKind::Spider | MobKind::Creeper
        )
    }

    /// Horizontal movement speed in blocks per tick.
    pub fn speed(self) -> f64 {
        match self {
            MobKind::Spider => 0.22,
            MobKind::Skeleton | MobKind::Zombie => 0.16,
            MobKind::Creeper => 0.14,
            MobKind::Chicken => 0.12,
            _ => 0.13,
        }
    }

    /// Melee damage dealt to a player (half-hearts).
    pub fn attack_damage(self) -> f32 {
        match self {
            MobKind::Zombie => 3.0,
            MobKind::Spider => 2.0,
            MobKind::Skeleton => 2.0,
            MobKind::Creeper => 6.0,
            _ => 0.0,
        }
    }

    /// Parse a kind from its lowercase name.
    pub fn from_name(name: &str) -> Option<MobKind> {
        MobKind::ALL.into_iter().find(|k| k.name() == name)
    }
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
    fn kinds_have_distinct_type_ids() {
        let mut ids: Vec<i32> = MobKind::ALL.iter().map(|k| k.type_id()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), MobKind::ALL.len());
    }

    #[test]
    fn hostility_and_lookup() {
        assert!(MobKind::Zombie.hostile());
        assert!(!MobKind::Cow.hostile());
        assert_eq!(MobKind::from_name("creeper"), Some(MobKind::Creeper));
        assert_eq!(MobKind::from_name("dragon"), None);
    }

    #[test]
    fn mob_starts_alive_at_full_health() {
        let m = Mob::new(7, MobKind::Pig, 0.0, 64.0, 0.0, 0.0);
        assert!(m.alive());
        assert_eq!(m.health, MobKind::Pig.max_health());
    }
}
