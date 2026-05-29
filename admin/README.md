# cubeplane admin panel

An [Atlas](https://github.com/wess/atlas)-powered control panel for the
cubeplane Minecraft engine. It runs on [Bun](https://bun.sh) and uses
`@atlas/server` for routing.

The panel is a **backend-for-frontend**: the browser only ever talks to the
panel, and the panel talks to the engine's control API server-side. That keeps
the optional control token off the client and avoids CORS.

```
browser ──▶ admin panel (Bun + @atlas/server) ──▶ cubeplane engine control API
                                                    (Rust, http://127.0.0.1:8080)
```

## Run

```bash
cd admin
bun install            # pulls Atlas from GitHub (see package.json)
cp .env.example .env   # optional; defaults work for a local engine
bun run dev            # http://localhost:3000
```

Make sure the engine is running with `[control] enabled = true` (the default).

## What it shows

- Live status: players online/max, uptime, generator, total joins, gamemode.
- Online players with positions and one-click **kick**.
- Loaded JS mods.
- **Broadcast** a chat message to everyone.
- **Set block** at world coordinates by name.

State refreshes every two seconds.

## Endpoints (proxied to the engine)

| Method | Path             | Purpose                         |
| ------ | ---------------- | ------------------------------- |
| GET    | `/`              | Dashboard (HTML)                |
| GET    | `/api/status`    | Server status                   |
| GET    | `/api/players`   | Connected players               |
| POST   | `/api/say`       | Broadcast a message             |
| POST   | `/api/kick`      | Kick a player                   |
| POST   | `/api/setblock`  | Place a block                   |

## Configuration

See `.env.example`. `ENGINE_URL` points at the engine control API;
`ENGINE_TOKEN` must match `control.token` in `cubeplane.toml` if set.
