// The single-page dashboard served at `/`. Self-contained HTML/CSS/JS that
// polls the panel's proxy endpoints and renders live server state.

export const dashboard = (): string => /* html */ `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>cubeplane · control panel</title>
<style>
  :root {
    --bg: #0e1116; --panel: #161b22; --border: #272e38; --text: #d7dde5;
    --muted: #8b97a7; --accent: #4ade80; --accent2: #38bdf8; --danger: #f87171;
  }
  * { box-sizing: border-box; }
  body { margin: 0; font: 14px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace;
    background: var(--bg); color: var(--text); }
  header { display: flex; align-items: center; gap: 12px; padding: 16px 24px;
    border-bottom: 1px solid var(--border); background: var(--panel); }
  header h1 { font-size: 18px; margin: 0; letter-spacing: .5px; }
  header .dot { width: 10px; height: 10px; border-radius: 50%; background: var(--danger);
    box-shadow: 0 0 8px var(--danger); transition: all .3s; }
  header .dot.online { background: var(--accent); box-shadow: 0 0 8px var(--accent); }
  header .ver { color: var(--muted); font-size: 12px; }
  main { max-width: 1000px; margin: 0 auto; padding: 24px; display: grid; gap: 20px; }
  .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: 14px; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: 10px; padding: 16px; }
  .card h2 { margin: 0 0 12px; font-size: 12px; text-transform: uppercase;
    letter-spacing: 1px; color: var(--muted); }
  .stat { font-size: 26px; font-weight: 600; color: var(--accent2); }
  .stat small { font-size: 13px; color: var(--muted); }
  ul { list-style: none; margin: 0; padding: 0; }
  li.player { display: flex; justify-content: space-between; align-items: center;
    padding: 8px 10px; border: 1px solid var(--border); border-radius: 8px; margin-bottom: 8px; }
  li.player .meta { color: var(--muted); font-size: 12px; }
  .tag { display: inline-block; background: #1f2733; border: 1px solid var(--border);
    border-radius: 6px; padding: 2px 8px; margin: 2px; font-size: 12px; color: var(--accent); }
  button { font: inherit; cursor: pointer; border: 1px solid var(--border);
    background: #1f2733; color: var(--text); border-radius: 8px; padding: 8px 14px; }
  button:hover { border-color: var(--accent2); }
  button.danger { color: var(--danger); }
  input { font: inherit; background: #0b0f14; border: 1px solid var(--border);
    color: var(--text); border-radius: 8px; padding: 8px 10px; }
  form.row { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; }
  form.row input { flex: 1; min-width: 100px; }
  .muted { color: var(--muted); }
  .empty { color: var(--muted); font-style: italic; }
</style>
</head>
<body>
<header>
  <span class="dot" id="dot"></span>
  <h1>cubeplane</h1>
  <span class="ver" id="ver">connecting…</span>
</header>
<main>
  <div class="grid">
    <div class="card"><h2>Players</h2><div class="stat"><span id="online">–</span><small>/<span id="max">–</span></small></div></div>
    <div class="card"><h2>Uptime</h2><div class="stat" id="uptime">–</div></div>
    <div class="card"><h2>Mobs</h2><div class="stat" id="mobs">–</div></div>
    <div class="card"><h2>Generator</h2><div class="stat" id="generator" style="font-size:18px">–</div></div>
    <div class="card"><h2>Total joins</h2><div class="stat" id="joins">–</div></div>
  </div>

  <div class="card">
    <h2>Online players</h2>
    <ul id="players"><li class="empty">nobody online</li></ul>
  </div>

  <div class="card">
    <h2>Loaded mods</h2>
    <div id="mods" class="muted">–</div>
  </div>

  <div class="card">
    <h2>Broadcast message</h2>
    <form class="row" id="sayForm">
      <input id="sayInput" placeholder="Message to all players…" autocomplete="off" />
      <button type="submit">Send</button>
    </form>
  </div>

  <div class="card">
    <h2>Set block</h2>
    <form class="row" id="blockForm">
      <input id="bx" type="number" placeholder="x" value="0" />
      <input id="by" type="number" placeholder="y" value="5" />
      <input id="bz" type="number" placeholder="z" value="0" />
      <input id="bname" placeholder="block (e.g. stone)" value="stone" />
      <button type="submit">Place</button>
    </form>
    <div id="blockMsg" class="muted" style="margin-top:8px"></div>
  </div>
</main>
<script>
const $ = (id) => document.getElementById(id);

function fmtUptime(s) {
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), sec = s % 60;
  return (h ? h + "h " : "") + (m ? m + "m " : "") + sec + "s";
}

async function refresh() {
  try {
    const st = await fetch("/api/status").then(r => r.json());
    $("dot").classList.add("online");
    $("ver").textContent = "Minecraft " + st.version + " · protocol " + st.protocol + " · " + st.gamemode;
    $("online").textContent = st.online;
    $("max").textContent = st.max;
    $("uptime").textContent = fmtUptime(st.uptimeSecs);
    $("mobs").textContent = st.mobCount ?? 0;
    $("generator").textContent = st.generator;
    $("joins").textContent = st.totalJoins;
    $("mods").innerHTML = (st.mods && st.mods.length)
      ? st.mods.map(m => '<span class="tag">' + m + "</span>").join("")
      : '<span class="empty">none</span>';

    const pl = await fetch("/api/players").then(r => r.json());
    const ul = $("players");
    if (!pl.players || pl.players.length === 0) {
      ul.innerHTML = '<li class="empty">nobody online</li>';
    } else {
      ul.innerHTML = pl.players.map(p =>
        '<li class="player"><span>' + p.name +
        ' <span class="meta">(' + p.x.toFixed(0) + ", " + p.y.toFixed(0) + ", " + p.z.toFixed(0) + ')</span></span>' +
        '<button class="danger" onclick="kick(\\'' + p.name + '\\')">kick</button></li>'
      ).join("");
    }
  } catch (e) {
    $("dot").classList.remove("online");
    $("ver").textContent = "engine offline";
  }
}

async function kick(name) {
  await fetch("/api/kick", { method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ player: name, reason: "Kicked from the control panel" }) });
  refresh();
}

$("sayForm").addEventListener("submit", async (e) => {
  e.preventDefault();
  const message = $("sayInput").value.trim();
  if (!message) return;
  await fetch("/api/say", { method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ message }) });
  $("sayInput").value = "";
});

$("blockForm").addEventListener("submit", async (e) => {
  e.preventDefault();
  const body = { x: +$("bx").value, y: +$("by").value, z: +$("bz").value, block: $("bname").value.trim() };
  const res = await fetch("/api/setblock", { method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify(body) });
  const data = await res.json();
  $("blockMsg").textContent = res.ok ? "Placed " + body.block + "." : ("Error: " + (data.error || res.status));
});

refresh();
setInterval(refresh, 2000);
</script>
</body>
</html>`;
