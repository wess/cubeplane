//! The "registry codec" NBT sent in the Login (Play) packet.
//!
//! Protocol 763 requires the server to ship the dimension-type, biome and
//! chat-type registries up front. The vanilla client validates that every
//! required field is present, so the dimension type below is complete. We ship
//! a single overworld dimension, a single `plains` biome (id 1, matching
//! [`cubeplane_world::chunk::DEFAULT_BIOME`]) and the seven vanilla chat types.

use cubeplane_nbt::{Nbt, Value};

/// Identifier of the dimension type players spawn in.
pub const DIMENSION_TYPE: &str = "minecraft:overworld";
/// Identifier of the world players spawn in.
pub const DIMENSION_NAME: &str = "minecraft:overworld";

/// All dimension identifiers, indexed by dimension number (0/1/2).
pub const DIMENSIONS: [&str; 3] =
    ["minecraft:overworld", "minecraft:the_nether", "minecraft:the_end"];

/// The dimension-type / world identifier for a dimension number.
pub fn dim_id(dim: u8) -> &'static str {
    DIMENSIONS[(dim as usize).min(2)]
}

/// Build the full registry codec compound.
pub fn codec() -> Nbt {
    Nbt::compound()
        .put_compound("minecraft:dimension_type", dimension_registry())
        .put_compound("minecraft:worldgen/biome", biome_registry())
        .put_compound("minecraft:chat_type", chat_type_registry())
}

fn registry_wrapper(type_name: &str, entries: Vec<Value>) -> Nbt {
    Nbt::compound()
        .put_string("type", type_name)
        .put_list("value", entries)
}

/// One dimension-type element. All dimensions reuse cubeplane's −64..320 chunk
/// shape so a single chunk encoder serves them; only the flavour differs.
fn dim_element(skylight: bool, ceiling: bool, ultrawarm: bool, ambient: f32, effects: &str) -> Nbt {
    Nbt::compound()
        .put_bool("piglin_safe", ultrawarm)
        .put_bool("has_raids", skylight)
        .put_int("monster_spawn_light_level", 0)
        .put_int("monster_spawn_block_light_limit", 0)
        .put_bool("natural", !ceiling)
        .put_float("ambient_light", ambient)
        .put_string("infiniburn", "#minecraft:infiniburn_overworld")
        .put_bool("respawn_anchor_works", ultrawarm)
        .put_bool("has_skylight", skylight)
        .put_bool("bed_works", skylight)
        .put_string("effects", effects)
        .put_int("min_y", cubeplane_world::chunk::MIN_Y)
        .put_int("height", cubeplane_world::chunk::WORLD_HEIGHT)
        .put_int("logical_height", cubeplane_world::chunk::WORLD_HEIGHT)
        .put_double("coordinate_scale", 1.0)
        .put_bool("ultrawarm", ultrawarm)
        .put_bool("has_ceiling", ceiling)
}

fn dimension_registry() -> Nbt {
    let dims = [
        ("minecraft:overworld", dim_element(true, false, false, 0.0, "minecraft:overworld")),
        ("minecraft:the_nether", dim_element(false, true, true, 0.1, "minecraft:the_nether")),
        ("minecraft:the_end", dim_element(false, false, false, 0.0, "minecraft:the_end")),
    ];
    let entries = dims
        .into_iter()
        .enumerate()
        .map(|(id, (name, element))| {
            Nbt::compound()
                .put_string("name", name)
                .put_int("id", id as i32)
                .put_compound("element", element)
                .into_value()
        })
        .collect();
    registry_wrapper("minecraft:dimension_type", entries)
}

fn biome_registry() -> Nbt {
    let effects = Nbt::compound()
        .put_int("sky_color", 0x78A7FF)
        .put_int("water_fog_color", 0x050533)
        .put_int("fog_color", 0xC0D8FF)
        .put_int("water_color", 0x3F76E4);

    let element = Nbt::compound()
        .put_bool("has_precipitation", true)
        .put_float("temperature", 0.8)
        .put_float("downfall", 0.4)
        .put_compound("effects", effects);

    let entry = Nbt::compound()
        .put_string("name", "minecraft:plains")
        .put_int("id", 1)
        .put_compound("element", element)
        .into_value();

    registry_wrapper("minecraft:worldgen/biome", vec![entry])
}

fn chat_type_registry() -> Nbt {
    // (id, registry name, chat translation key, narration translation key)
    let types = [
        (0, "minecraft:chat", "chat.type.text", "chat.type.text.narrate"),
        (1, "minecraft:say_command", "chat.type.announcement", "chat.type.text.narrate"),
        (2, "minecraft:msg_command_incoming", "commands.message.display.incoming", "chat.type.text.narrate"),
        (3, "minecraft:msg_command_outgoing", "commands.message.display.outgoing", "chat.type.text.narrate"),
        (4, "minecraft:team_msg_command_incoming", "chat.type.team.text", "chat.type.text.narrate"),
        (5, "minecraft:team_msg_command_outgoing", "chat.type.team.sent", "chat.type.text.narrate"),
        (6, "minecraft:emote_command", "chat.type.emote", "chat.type.emote"),
    ];

    let entries = types
        .iter()
        .map(|(id, name, chat_key, narrate_key)| {
            let decoration = |key: &str| {
                Nbt::compound()
                    .put_string("translation_key", key)
                    .put_list(
                        "parameters",
                        vec![Value::String("sender".into()), Value::String("content".into())],
                    )
                    .put_compound("style", Nbt::compound())
            };
            let element = Nbt::compound()
                .put_compound("chat", decoration(chat_key))
                .put_compound("narration", decoration(narrate_key));
            Nbt::compound()
                .put_string("name", *name)
                .put_int("id", *id)
                .put_compound("element", element)
                .into_value()
        })
        .collect();

    registry_wrapper("minecraft:chat_type", entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_serializes() {
        let bytes = codec().to_bytes_named("");
        // A complete codec is comfortably over a kilobyte.
        assert!(bytes.len() > 500, "codec unexpectedly small: {}", bytes.len());
    }
}
