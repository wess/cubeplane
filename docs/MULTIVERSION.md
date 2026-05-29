# Multi-version support

cubeplane builds every packet against the canonical **protocol 763 (1.20.1)**
layout, then translates the byte stream to the client's version. The whole
translation layer lives in [`crates/server/src/version.rs`](../crates/server/src/version.rs);
the negotiation/registry lives in [`crates/protocol/src/lib.rs`](../crates/protocol/src/lib.rs).

## Supported versions

`SUPPORTED_PROTOCOLS` (in `crates/protocol/src/lib.rs`) — connectable today:

| Protocol | Minecraft | Notes |
| --- | --- | --- |
| 757 | 1.18 / 1.18.1 | identical wire format to 1.18.2 |
| 758 | 1.18.2 | pre-1.19 login (dimension as NBT), no system_chat |
| 759 | 1.19 / 1.19.1 | bodies identical to 1.19.2, different ids |
| 760 | 1.19.2 | old `player_info` (enum action) |
| 761 | 1.19.3 | action-bitmask `player_info` (like 763); `position` +dismount |
| 762 | 1.19.4 | play ids identical to 763; login drops `portalCooldown` |
| **763** | **1.20.1** | **native — no translation** |
| 764 | 1.20.2 | adds Configuration phase; network NBT becomes nameless |
| 765 | 1.20.3 / 1.20.4 | JSON chat → NBT text components |

`KNOWN_VERSIONS` additionally names 47…767 so the login gate gives a clean,
version-named "please use 1.20.1" message to unsupported clients, and the status
ping echoes a compatible version when possible.

## How translation works

Two entry points, both threaded with the negotiated `protocol`:

- **Clientbound** — `translate_clientbound(payload, protocol)`, called in the
  per-player writer task in `connection.rs::play`. It reads the leading packet-id
  varint, remaps it (`remap_play_clientbound`), then runs
  `rewrite_clientbound_body` for packets whose body layout changed.
- **Serverbound** — `translate_serverbound(frame, protocol)`, called in
  `play_loop` before `RawPacket::parse`. Mirror of the above.

Native 763 is a pass-through (identity), so it has zero overhead and is never
touched.

### Id maps

Per-version `CB_763_TO_xxx` / `SB_xxx_TO_763` tables, generated from
minecraft-data (see recipe below). `apply_map` does a sparse lookup; unlisted
ids pass through unchanged.

### Body rewriters (the hard part)

Keystone transforms, all in `version.rs`:

| Transform | Versions | Function |
| --- | --- | --- |
| named NBT → anonymous (network NBT) | 1.20.2+ | `named_root_nbt_to_anonymous` |
| JSON chat string → NBT text component | 1.20.3+ | `chat_json_to_anonymous_nbt` / `json_value_to_nbt` |
| `trustEdges` insert in chunk/light | 1.18–1.19.4 | `rewrite_clientbound_body_119x` |
| `player_info` bitmask→enum + fold `player_remove` | 1.18–1.19.2 | `convert_player_info_add_763_to_old`, `convert_player_remove_763_to_old` |
| login: drop `portalCooldown` | 1.19.x | `rewrite_login_763_to_762` |
| login: reorder + drop codec + `doLimitedCrafting` | 1.20.2/1.20.3 | `rewrite_login_763_to_764` |
| login: dimension inline as NBT | 1.18 | `rewrite_login_763_to_758` |
| `system_chat` → old `chat` packet | 1.18 | inline in `rewrite_clientbound_body_758` |

### Configuration phase (1.20.2+)

`finish_login` calls `configuration_phase` when `protocol >= 764`: await Login
Acknowledged (login sb `0x03`), send Registry Data (`cb::config_registry_data`,
the codec as nameless NBT) + Finish Configuration, then await the client's
Finish ack before entering Play.

## Recipe: add an adjacent modern version (1.18+)

This is mechanical. Example workflow used for every version 757–765:

```bash
cd /tmp
# 1. Resolve the data path (minecraft-data shares folders across versions)
curl -sS https://raw.githubusercontent.com/PrismarineJS/minecraft-data/master/data/dataPaths.json -o dp.json
jq -r '.pc["1.20.4"].protocol' dp.json     # -> the folder, e.g. pc/1.20.3

# 2. Fetch that version's protocol.json (and 1.20 = 763 as the canonical)
curl -sS .../data/pc/1.20/protocol.json -o p763.json
curl -sS .../data/pc/<ver>/protocol.json -o pXXX.json

# 3. Diff the clientbound play mappers to generate the id map, and list
#    packets whose BODY differs (those need rewriters):
python3 - <<'PY'
import json
def m(p,d): return {n:int(k,16) for k,n in json.load(open(p))["play"][d]["types"]["packet"][1][0]["type"][1]["mappings"].items()}
a=json.load(open("p763.json"))["play"]["toClient"]["types"]
b=json.load(open("pXXX.json"))["play"]["toClient"]["types"]
common=[k for k in a if k.startswith("packet_") and k in b]
print("changed bodies:", [k.replace("packet_","") for k in common if json.dumps(a[k])!=json.dumps(b[k])])
# ... emit (canonical_id, wire_id) pairs from m(...) diffs
PY
```

Then in `version.rs`: add a `PROTO_X` const, the `CB_763_TO_X`/`SB_X_TO_763`
maps, wire them into `remap_play_clientbound`/`remap_play_serverbound` and
`rewrite_clientbound_body`, add the version to `SUPPORTED_PROTOCOLS`, and add a
`version_X_join` integration test in `connection.rs` that drives a simulated
client and parses the Join Game (and any converted packet) to confirm it's
well-formed.

## Remaining ranges — status, evidence, and next steps

### 1.20.5–1.21.1 (766/767) — sourceable, large, type-sensitive

Geometry-compatible (height 384, min_y −64 — same as cubeplane; verified from
`pc/1.20.5` `dimension_type/overworld.json`). 1.20.5 reworked the config phase to
send **per-registry** `registry_data` packets (`{id, entries:[{key, value:opt nbt}]}`,
ids: registry_data `0x07`, finish `0x03`) covering all dynamic registries.

The registry content is **available** from misode/mcmeta:
```
https://raw.githubusercontent.com/misode/mcmeta/refs/tags/1.20.5-data/data/minecraft/<registry>/<entry>.json
```
(`damage_type/*`, `dimension_type/*`, `worldgen/biome/*`, `chat_type/*`,
`trim_pattern/*`, `trim_material/*`, `banner_pattern/*`, `wolf_variant/*`,
`painting_variant/*`).

**Why it's not done yet (the real blocker):** JSON carries no NBT type tags, and
the client validates types strictly. A generic JSON→NBT pass emits biome
`temperature` as `double`/`int` when the client requires `float` → disconnect.
Doing it correctly needs **per-field type tables per registry**, plus a real
1.20.5 client to validate against (not available in-sandbox). Don't ship a
guessed codec.

**Safe next step:** build the per-registry packet machinery + a vendored,
type-annotated registry codec; keep 766/767 *out* of `SUPPORTED_PROTOCOLS` until
validated against a real client (enabling it can't regress 763 — separate code
paths — but a half-right codec just disconnects 766 clients, same as a clean
reject, so there's no point shipping it unverified).

### 1.16–1.17 (754–756) — core world geometry

These use a `0..256` world (16 sections) and the pre-1.18 bitmask chunk format
with a separate biome array. cubeplane's world is `-64..320` (24 sections,
`crates/world/src/chunk.rs`). Supporting them is a **core-world change**
(a configurable geometry mode + a chunk-data transcoder), not a translation
adapter. 1.17 *might* accept `-64..384` via the dimension type, but still needs
the bitmask chunk re-encode + separated biomes.

### 1.8 (protocol 47) — separate implementation

Pre-flattening: all 1003 block *states* would remap to numeric block-id +
metadata, with an entirely different chunk format and entity metadata. This is a
ground-up second protocol backend — the scope of ViaRewind/ViaBackwards.

## Non-negotiable: never fabricate

All ids/maps/registry data come from authoritative sources
(`minecraft-data`, `misode/mcmeta`) fetched via `curl`+`jq`. A fabricated id or
registry field silently corrupts the stream and disconnects clients — strictly
worse than the clean version-named reject the server already gives. This
discipline caught a real 763 bug (the `advancements` packet was missing the
`criteria` array and `sendsTelemetryData` bool that 763 requires).
