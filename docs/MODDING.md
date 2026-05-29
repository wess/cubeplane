# Modding cubeplane

cubeplane mods are plain JavaScript files evaluated by an embedded
[QuickJS](https://bellard.org/quickjs/) runtime (via
[`rquickjs`](https://github.com/DelSkayn/rquickjs)). Each `.js` file in the
configured mods directory (`mods/` by default) is loaded once at startup.

Mods run on a dedicated thread, isolated from the engine's async runtime. They
**react to events** and **request actions**; the engine applies those actions
on the next pass. There is no blocking call back into the engine, which keeps
mods snappy and crash-isolated — a throwing handler is logged, not fatal.

## The `cubeplane` global

Every mod has access to a global `cubeplane` object.

### Registering handlers

```js
cubeplane.on(eventName, (event) => { /* ... */ });
cubeplane.command(name, (ctx) => { /* ... */ });   // ctx = { player, command, args }
```

### Actions

| Call | Effect |
| --- | --- |
| `cubeplane.broadcast(message)` | Send a chat message to everyone |
| `cubeplane.tell(player, message)` | Send a chat message to one player |
| `cubeplane.log(message)` | Write a line to the server log |
| `cubeplane.setBlock(x, y, z, block)` | Place a block by name at world coords |
| `cubeplane.kick(player, reason)` | Disconnect a player |

`console.log` is also available and maps to `cubeplane.log`. Color chat with
Minecraft's `§` codes (e.g. `"§aGreen text"`).

## Events

| Event | Payload |
| --- | --- |
| `server_start` | `{ version }` |
| `server_stop` | `{}` |
| `tick` | `{ tick }` — fired once per second |
| `player_join` | `{ player, uuid, entityId }` |
| `player_leave` | `{ player }` |
| `chat` | `{ player, message }` |
| `command` | `{ player, command, args }` — only for commands with no registered handler |
| `block_place` | `{ player, x, y, z, block }` |
| `block_break` | `{ player, x, y, z }` |

## Commands

`cubeplane.command("greet", fn)` registers `/greet`. Built-in commands
(`/help`, `/list`, `/pos`, `/tp`) are handled by the engine first; any other
slash command is routed to a matching mod command, or — if none matches — to
the generic `command` event.

```js
cubeplane.command("home", (ctx) => {
  cubeplane.tell(ctx.player, "There's no place like /home.");
});
```

## Block names

`setBlock` and `block_place` use the curated block names in the registry:

`air`, `stone`, `granite`, `polished_andesite`, `grass_block`, `dirt`,
`coarse_dirt`, `podzol`, `cobblestone`, `oak_planks`, `bedrock`, `water`,
`sand`, `gravel`, `oak_log`, `oak_leaves`, `glass`, `lapis_block`.

Unknown names are ignored (for `setBlock`) so a typo can't crash anything.

## A complete example

```js
// scoreboard.js — track and announce blocks placed per player.
const counts = {};

cubeplane.on("player_join", (e) => { counts[e.player] = counts[e.player] || 0; });

cubeplane.on("block_place", (e) => {
  counts[e.player] = (counts[e.player] || 0) + 1;
  if (counts[e.player] % 25 === 0) {
    cubeplane.broadcast("§6" + e.player + " has placed " + counts[e.player] + " blocks!");
  }
});

cubeplane.command("blocks", (ctx) => {
  cubeplane.tell(ctx.player, "You've placed " + (counts[ctx.player] || 0) + " blocks.");
});
```

See [`mods/`](../mods) for the bundled `welcome.js`, `builder.js` and
`playground.js`.

## Limits & notes

- The runtime has a 64 MiB memory cap.
- Handlers should return quickly; heavy loops block other mods (they share one
  thread).
- Actions are applied in order, shortly after the handler returns.
- Mods cannot (yet) read world state synchronously — they observe via events
  and mutate via actions.
