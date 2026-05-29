//! Minimal helpers for building Minecraft *text components* (chat JSON).
//!
//! The protocol carries chat, disconnect reasons and the player list as JSON
//! text components. We only need a small slice of the format, so these helpers
//! emit `serde_json::Value` directly rather than pulling in a full model.

use serde_json::{json, Value};

/// A plain white text component.
pub fn plain(text: impl Into<String>) -> Value {
    json!({ "text": text.into() })
}

/// A colored text component. `color` is a vanilla color name (e.g. "yellow").
pub fn colored(text: impl Into<String>, color: &str) -> Value {
    json!({ "text": text.into(), "color": color })
}

/// Standard `<name> message` chat line, with a gray name and white body.
pub fn chat_line(name: &str, message: &str) -> Value {
    json!({
        "text": "",
        "extra": [
            { "text": format!("<{name}> "), "color": "gray" },
            { "text": message, "color": "white" }
        ]
    })
}

/// A yellow server announcement, e.g. join/leave notifications.
pub fn system_notice(message: impl Into<String>) -> Value {
    colored(message, "yellow")
}

/// Serialize a component to the compact JSON string sent on the wire.
pub fn to_string(value: &Value) -> String {
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_line_shape() {
        let v = chat_line("wess", "hello");
        assert_eq!(v["extra"][0]["text"], "<wess> ");
        assert!(to_string(&v).contains("hello"));
    }
}
