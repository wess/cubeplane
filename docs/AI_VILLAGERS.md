# AI villagers (experimental)

An optional feature that gives villagers real, in-character conversations driven
by an LLM. It is **off by default** and only ever reaches the network when
enabled.

## How it works

- Every villager is assigned a stable **name and profession** (farmer,
  librarian, blacksmith, cleric, cartographer, fisherman, fletcher, shepherd,
  butcher, mason). When the feature is on, that shows as a floating nameplate.
- **Right-click a villager** to start talking. Your chat messages then go to
  that villager instead of public chat, and it replies in character. Say
  `bye` (or walk off and re-engage) to end the conversation.
- Each villager keeps a short rolling memory of your exchange, and only one
  request per villager is in flight at a time.
- With the feature **off**, right-clicking a villager opens the normal trade
  window instead.

## Providers

cubeplane speaks to three backends through one `chat()` abstraction:

| Provider | Endpoint | Notes |
| --- | --- | --- |
| `ollama` | `POST {base}/api/chat` | Local, free, no key. Default `base` `http://localhost:11434`. |
| `openai` | `POST {base}/v1/chat/completions` | Needs an API key. Works with any OpenAI-compatible server via `base_url`. |
| `claude` | `POST {base}/v1/messages` | Anthropic Messages API; needs an API key. |

## Configuring

Either edit `[ai]` in `cubeplane.toml`:

```toml
[ai]
enabled = true
provider = "ollama"      # ollama | openai | claude
model = "llama3.2"
# api_key = "..."        # required for openai / claude
# base_url = "..."       # override endpoint; empty = provider default
max_tokens = 200
history_limit = 6
temperature = 0.8
```

…or toggle it **live from the Atlas admin panel** (no restart): the *AI
villagers* card exposes the enable switch, provider, model, base URL and key.
The key is write-only — the panel only ever reports whether one is set, never
its value.

## Notes & limits

- Replies are best-effort: on any provider/network error the villager simply
  mutters and the conversation continues.
- Conversations are in-memory and reset on restart.
- Trade economy is not simulated while the trade window is open (read-only).
