//! Experimental LLM-backed villagers.
//!
//! Each villager is given a profession; talking to one sends the conversation
//! to the configured provider (Ollama, OpenAI or Claude) with a role-flavoured
//! system prompt, and the reply is spoken back in chat. All network calls are
//! async and gated behind [`AiConfig::enabled`], so the feature is entirely
//! opt-in and never touches the network when off.

use anyhow::{anyhow, Context};
use serde_json::{json, Value};

use crate::config::AiConfig;

/// One turn of dialogue.
#[derive(Debug, Clone)]
pub struct Turn {
    /// "user" (the player) or "assistant" (the villager).
    pub role: &'static str,
    pub text: String,
}

/// The villager professions we assign and role-play.
pub const PROFESSIONS: &[&str] = &[
    "farmer",
    "librarian",
    "blacksmith",
    "cleric",
    "cartographer",
    "fisherman",
    "fletcher",
    "shepherd",
    "butcher",
    "mason",
];

/// A pool of villager given-names.
const NAMES: &[&str] = &[
    "Edda", "Bram", "Tilda", "Goran", "Mira", "Olen", "Sefa", "Dunstan", "Petra", "Cuthbert",
    "Wren", "Halla", "Ferro", "Lysa", "Odo", "Magda",
];

/// Pick a profession deterministically from an entity id (stable per villager).
pub fn profession_for(entity_id: i32) -> &'static str {
    let idx = (entity_id.unsigned_abs() as usize) % PROFESSIONS.len();
    PROFESSIONS[idx]
}

/// Pick a stable given-name for a villager.
pub fn name_for(entity_id: i32) -> String {
    let idx = (entity_id.unsigned_abs() as usize / 7) % NAMES.len();
    NAMES[idx].to_string()
}

/// Build the system prompt that gives a villager its personality.
pub fn system_prompt(name: &str, profession: &str) -> String {
    format!(
        "You are {name}, a {profession} living in a Minecraft village. Stay fully in \
         character as a medieval village {profession}. Speak in first person, warmly and \
         with a little whimsy. Keep replies to one to three short sentences — this is \
         spoken dialogue in a game chat box. Never break character, never mention being \
         an AI, and don't use markdown. Reference your trade ({profession} life, the \
         village, the world around you) when it fits."
    )
}

/// Call the configured provider and return the villager's reply text.
pub async fn chat(cfg: &AiConfig, system: &str, history: &[Turn], user: &str) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    match cfg.provider.as_str() {
        "openai" => openai(&client, cfg, system, history, user).await,
        "claude" => claude(&client, cfg, system, history, user).await,
        _ => ollama(&client, cfg, system, history, user).await,
    }
}

/// OpenAI-compatible `/v1/chat/completions` (also used by many local servers).
async fn openai(client: &reqwest::Client, cfg: &AiConfig, system: &str, history: &[Turn], user: &str) -> anyhow::Result<String> {
    let mut messages = vec![json!({"role": "system", "content": system})];
    for t in history {
        messages.push(json!({"role": t.role, "content": t.text}));
    }
    messages.push(json!({"role": "user", "content": user}));

    let url = format!("{}/v1/chat/completions", cfg.effective_base_url());
    let body = json!({
        "model": cfg.model,
        "messages": messages,
        "max_tokens": cfg.max_tokens,
        "temperature": cfg.temperature,
    });
    let resp = client
        .post(url)
        .bearer_auth(&cfg.api_key)
        .json(&body)
        .send()
        .await?
        .error_for_status()
        .context("openai request failed")?;
    let v: Value = resp.json().await?;
    extract_openai(&v)
}

/// Parse an OpenAI chat-completion response.
fn extract_openai(v: &Value) -> anyhow::Result<String> {
    v["choices"][0]["message"]["content"]
        .as_str()
        .map(clean)
        .ok_or_else(|| anyhow!("no choices in response"))
}

/// Anthropic Messages API.
async fn claude(client: &reqwest::Client, cfg: &AiConfig, system: &str, history: &[Turn], user: &str) -> anyhow::Result<String> {
    let mut messages = Vec::new();
    for t in history {
        messages.push(json!({"role": t.role, "content": t.text}));
    }
    messages.push(json!({"role": "user", "content": user}));

    let url = format!("{}/v1/messages", cfg.effective_base_url());
    let body = json!({
        "model": cfg.model,
        "system": system,
        "messages": messages,
        "max_tokens": cfg.max_tokens,
        "temperature": cfg.temperature,
    });
    let resp = client
        .post(url)
        .header("x-api-key", &cfg.api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await?
        .error_for_status()
        .context("claude request failed")?;
    let v: Value = resp.json().await?;
    extract_claude(&v)
}

/// Parse an Anthropic Messages response (`content` is an array of blocks).
fn extract_claude(v: &Value) -> anyhow::Result<String> {
    v["content"][0]["text"]
        .as_str()
        .map(clean)
        .ok_or_else(|| anyhow!("no content in response"))
}

/// Ollama `/api/chat`.
async fn ollama(client: &reqwest::Client, cfg: &AiConfig, system: &str, history: &[Turn], user: &str) -> anyhow::Result<String> {
    let mut messages = vec![json!({"role": "system", "content": system})];
    for t in history {
        messages.push(json!({"role": t.role, "content": t.text}));
    }
    messages.push(json!({"role": "user", "content": user}));

    let url = format!("{}/api/chat", cfg.effective_base_url());
    let body = json!({
        "model": cfg.model,
        "messages": messages,
        "stream": false,
        "options": { "temperature": cfg.temperature, "num_predict": cfg.max_tokens },
    });
    let resp = client
        .post(url)
        .json(&body)
        .send()
        .await?
        .error_for_status()
        .context("ollama request failed")?;
    let v: Value = resp.json().await?;
    extract_ollama(&v)
}

/// Parse an Ollama chat response.
fn extract_ollama(v: &Value) -> anyhow::Result<String> {
    v["message"]["content"]
        .as_str()
        .map(clean)
        .ok_or_else(|| anyhow!("no message in response"))
}

/// Trim and collapse model output to a single tidy chat line.
fn clean(s: &str) -> String {
    let s = s.trim();
    // Collapse internal newlines so it fits the chat box.
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn professions_are_stable() {
        assert_eq!(profession_for(5), profession_for(5));
        assert!(PROFESSIONS.contains(&profession_for(123)));
    }

    #[test]
    fn parsers_extract_text() {
        let oa = json!({"choices":[{"message":{"content":" Hello\nthere "}}]});
        assert_eq!(extract_openai(&oa).unwrap(), "Hello there");
        let cl = json!({"content":[{"type":"text","text":"Greetings, traveller."}]});
        assert_eq!(extract_claude(&cl).unwrap(), "Greetings, traveller.");
        let ol = json!({"message":{"role":"assistant","content":"Aye, good day!"}});
        assert_eq!(extract_ollama(&ol).unwrap(), "Aye, good day!");
    }

    #[tokio::test]
    async fn chat_round_trips_against_a_mock_provider() {
        use axum::{routing::post, Json, Router};
        // A mock OpenAI-compatible endpoint that echoes a canned completion.
        let app = Router::new().route(
            "/v1/chat/completions",
            post(|Json(_body): Json<serde_json::Value>| async {
                Json(json!({
                    "choices": [{"message": {"role": "assistant", "content": "Aye, well met, traveller!"}}]
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let cfg = AiConfig {
            enabled: true,
            provider: "openai".into(),
            model: "mock".into(),
            api_key: "test".into(),
            base_url: format!("http://127.0.0.1:{port}"),
            ..Default::default()
        };
        let reply = chat(&cfg, &system_prompt("Edda", "farmer"), &[], "Hello!")
            .await
            .unwrap();
        assert_eq!(reply, "Aye, well met, traveller!");
    }

    #[test]
    fn base_url_defaults() {
        let mut c = AiConfig { provider: "openai".into(), ..Default::default() };
        assert_eq!(c.effective_base_url(), "https://api.openai.com");
        c.provider = "ollama".into();
        assert_eq!(c.effective_base_url(), "http://localhost:11434");
        c.base_url = "http://127.0.0.1:1234/".into();
        assert_eq!(c.effective_base_url(), "http://127.0.0.1:1234");
    }
}
