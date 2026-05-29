// Runtime configuration for the admin panel, read from the environment.
// The panel is a backend-for-frontend: the browser talks only to the panel,
// and the panel talks to the cubeplane engine's control API server-side. That
// keeps the control token off the client and sidesteps CORS.

export const config = {
  /** Port the admin panel listens on. */
  port: Number(process.env.PORT ?? 3000),
  /** Interface the admin panel binds. */
  host: process.env.HOST ?? "0.0.0.0",
  /** Base URL of the cubeplane engine control API. */
  engineUrl: (process.env.ENGINE_URL ?? "http://127.0.0.1:8080").replace(/\/$/, ""),
  /** Optional bearer token required by the engine control API. */
  engineToken: process.env.ENGINE_TOKEN ?? "",
};
