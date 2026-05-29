//! A small advancement tree and the Update Advancements packet (protocol 763 /
//! 1.20.1). The packet format is followed exactly for 763 — in particular the
//! `sends_telemetry` boolean (added in 1.20.2) is intentionally omitted.
//!
//! Each advancement has a single criterion equal to its key's last segment, so
//! awarding one is just a matter of marking that criterion achieved.

use bytes::BytesMut;
use cubeplane_protocol::ProtoWrite;

use crate::clientbound::{pkt, write_slot};
use crate::ids::play_cb;
use crate::item::{self, ItemStack};

/// One node in the advancement tree.
struct Node {
    key: &'static str,
    parent: Option<&'static str>,
    title: &'static str,
    description: &'static str,
    icon_item: &'static str,
    /// Frame: 0 = task, 1 = challenge, 2 = goal.
    frame: i32,
    x: f32,
    y: f32,
    /// Background texture (root only).
    background: Option<&'static str>,
}

/// The cubeplane advancement tree: a root tab plus a few milestones.
const TREE: &[Node] = &[
    Node {
        key: "cubeplane:root",
        parent: None,
        title: "cubeplane",
        description: "Welcome to cubeplane!",
        icon_item: "grass_block",
        frame: 0,
        x: 0.0,
        y: 0.0,
        background: Some("minecraft:textures/gui/advancements/backgrounds/stone.png"),
    },
    Node {
        key: "cubeplane:mine",
        parent: Some("cubeplane:root"),
        title: "Stone Age",
        description: "Mine your first block.",
        icon_item: "stone",
        frame: 0,
        x: 1.0,
        y: -1.0,
        background: None,
    },
    Node {
        key: "cubeplane:kill",
        parent: Some("cubeplane:root"),
        title: "Monster Hunter",
        description: "Kill a hostile mob.",
        icon_item: "iron_sword",
        frame: 0,
        x: 1.0,
        y: 0.0,
        background: None,
    },
    Node {
        key: "cubeplane:trade",
        parent: Some("cubeplane:root"),
        title: "What a Deal!",
        description: "Trade with a villager.",
        icon_item: "emerald",
        frame: 1,
        x: 1.0,
        y: 1.0,
        background: None,
    },
];

/// The criterion name for an advancement (the key's last path segment).
fn criterion(key: &str) -> &str {
    key.rsplit(':').next().unwrap_or(key)
}

/// Map a milestone event name to the advancement key it completes.
pub fn key_for_event(event: &str) -> Option<&'static str> {
    match event {
        "mine" => Some("cubeplane:mine"),
        "kill" => Some("cubeplane:kill"),
        "trade" => Some("cubeplane:trade"),
        _ => None,
    }
}

/// A title/description chat component as a 1.20.1 JSON string.
fn chat_json(text: &str) -> String {
    serde_json::json!({ "text": text }).to_string()
}

/// Build the Update Advancements packet defining the whole tree, marking every
/// key in `completed` as achieved.
pub fn packet(completed: &[&str]) -> BytesMut {
    let mut b = pkt(play_cb::UPDATE_ADVANCEMENTS);
    b.write_bool(true); // reset/clear before applying

    // Advancement mapping.
    b.write_varint(TREE.len() as i32);
    for node in TREE {
        b.write_string(node.key);

        // Optional parent.
        match node.parent {
            Some(p) => {
                b.write_bool(true);
                b.write_string(p);
            }
            None => b.write_bool(false),
        }

        // Display data is always present for our nodes.
        b.write_bool(true);
        b.write_string(&chat_json(node.title));
        b.write_string(&chat_json(node.description));
        // Icon slot.
        let icon = item::id_any(node.icon_item).map(|id| ItemStack::new(id, 1)).unwrap_or(ItemStack::EMPTY);
        write_slot(&mut b, icon);
        b.write_varint(node.frame);
        // Flags: 0x1 has background, 0x2 show toast, 0x4 hidden.
        let mut flags = 0x2; // show toast
        if node.background.is_some() {
            flags |= 0x1;
        }
        b.write_i32(flags);
        if let Some(bg) = node.background {
            b.write_string(bg);
        }
        b.write_f32(node.x);
        b.write_f32(node.y);

        // Requirements: a single group with this node's lone criterion.
        let crit = criterion(node.key);
        b.write_varint(1); // one requirement group
        b.write_varint(1); // one criterion in the group
        b.write_string(crit);
        // NB: protocol 763 has no `sends_telemetry` boolean here.
    }

    // Removed advancements: none.
    b.write_varint(0);

    // Progress mapping: report each node's criterion as done or not.
    b.write_varint(TREE.len() as i32);
    for node in TREE {
        b.write_string(node.key);
        let crit = criterion(node.key);
        b.write_varint(1); // one criterion's progress
        b.write_string(crit);
        if completed.contains(&node.key) {
            b.write_bool(true); // achieved
            b.write_i64(0); // date achieved (epoch millis; 0 is fine)
        } else {
            b.write_bool(false);
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn criterion_is_last_segment() {
        assert_eq!(criterion("cubeplane:mine"), "mine");
        assert_eq!(criterion("plain"), "plain");
    }

    #[test]
    fn events_map_to_keys() {
        assert_eq!(key_for_event("mine"), Some("cubeplane:mine"));
        assert_eq!(key_for_event("kill"), Some("cubeplane:kill"));
        assert_eq!(key_for_event("trade"), Some("cubeplane:trade"));
        assert_eq!(key_for_event("nope"), None);
    }

    #[test]
    fn packet_encodes_without_panic() {
        // Empty and partial completion both encode to a non-trivial buffer.
        assert!(packet(&[]).len() > 16);
        assert!(packet(&["cubeplane:mine"]).len() > packet(&[]).len() - 4);
        // Every tree node's icon resolves in the registry.
        for node in TREE {
            assert!(item::id_any(node.icon_item).is_some(), "{} icon missing", node.icon_item);
        }
        // Root has a background; children do not.
        assert!(TREE[0].background.is_some());
        assert!(TREE[1].background.is_none());
    }
}
