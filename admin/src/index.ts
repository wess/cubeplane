// cubeplane admin/control panel — built on @atlas/server.
//
// Atlas routes are plain values: `get`/`post` return a Route, and a handler is
// a PipeFn `(conn) => Conn`. We compose a small backend-for-frontend that
// serves the dashboard and proxies the engine control API.

import { get, json, post, putHeader, serve, type Conn } from "@atlas/server";

import { config } from "./config.ts";
import { dashboard } from "./dashboard.ts";
import { EngineError, engine } from "./engine.ts";

/** Respond with an HTML body. */
const html = (conn: Conn, body: string): Conn => ({
  ...putHeader(conn, "content-type", "text/html; charset=utf-8"),
  status: 200,
  body,
  halted: true,
});

/** Run an engine call, translating EngineError into a JSON error response. */
const proxy = async (conn: Conn, fn: () => Promise<unknown>): Promise<Conn> => {
  try {
    return json(conn, 200, await fn());
  } catch (err) {
    const status = err instanceof EngineError ? err.status : 500;
    return json(conn, status, { error: err instanceof Error ? err.message : "error" });
  }
};

/** Read a JSON request body, tolerating empty/invalid payloads. */
const readJson = async (conn: Conn): Promise<Record<string, any>> => {
  try {
    return (await conn.request.json()) as Record<string, any>;
  } catch {
    return {};
  }
};

const routes = [
  get("/", (conn) => html(conn, dashboard())),
  get("/healthz", (conn) => json(conn, 200, { ok: true })),

  get("/api/status", (conn) => proxy(conn, () => engine.status())),
  get("/api/players", (conn) => proxy(conn, () => engine.players())),

  post("/api/say", async (conn) => {
    const body = await readJson(conn);
    return proxy(conn, () => engine.say(String(body.message ?? "")));
  }),
  post("/api/kick", async (conn) => {
    const body = await readJson(conn);
    return proxy(conn, () => engine.kick(String(body.player ?? ""), body.reason));
  }),
  post("/api/setblock", async (conn) => {
    const body = await readJson(conn);
    return proxy(conn, () =>
      engine.setblock(Number(body.x), Number(body.y), Number(body.z), String(body.block ?? "")),
    );
  }),

  get("/api/ai", (conn) => proxy(conn, () => engine.ai())),
  post("/api/ai", async (conn) => {
    const body = await readJson(conn);
    return proxy(conn, () => engine.setAi(body));
  }),
];

serve({ port: config.port, hostname: config.host, routes });

console.log(`cubeplane admin panel → http://${config.host}:${config.port}`);
console.log(`  proxying engine control API at ${config.engineUrl}`);
