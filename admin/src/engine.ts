// Thin client for the cubeplane engine control API.

import { config } from "./config.ts";

const authHeaders = (): Record<string, string> => {
  const headers: Record<string, string> = { "content-type": "application/json" };
  if (config.engineToken) headers["authorization"] = `Bearer ${config.engineToken}`;
  return headers;
};

/** Raised when the engine cannot be reached or returns an error status. */
export class EngineError extends Error {
  constructor(
    message: string,
    public readonly status: number = 502,
  ) {
    super(message);
  }
}

const request = async (method: string, path: string, body?: unknown): Promise<unknown> => {
  let res: Response;
  try {
    res = await fetch(`${config.engineUrl}${path}`, {
      method,
      headers: authHeaders(),
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new EngineError("engine unreachable", 502);
  }
  const text = await res.text();
  const data = text ? JSON.parse(text) : {};
  if (!res.ok) throw new EngineError((data as any).error ?? "engine error", res.status);
  return data;
};

export const engine = {
  status: () => request("GET", "/api/status"),
  players: () => request("GET", "/api/players"),
  say: (message: string) => request("POST", "/api/say", { message }),
  kick: (player: string, reason?: string) => request("POST", "/api/kick", { player, reason }),
  setblock: (x: number, y: number, z: number, block: string) =>
    request("POST", "/api/setblock", { x, y, z, block }),
};
