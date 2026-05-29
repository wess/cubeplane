// cubeplane mod runtime prelude.
//
// This sets up the global `cubeplane` object that mods use to register event
// handlers and commands and to act on the world. Calls accumulate "actions"
// that the Rust host drains and executes after each dispatch, so the JS side
// never needs a live bridge back into the engine.

globalThis.__cubeplane = (function () {
  const handlers = Object.create(null);
  const commands = Object.create(null);
  let actions = [];

  const api = {
    /** Register a handler for an event (e.g. "player_join", "chat", "tick"). */
    on(event, fn) {
      if (typeof fn !== "function") throw new Error("on(event, fn): fn must be a function");
      (handlers[event] || (handlers[event] = [])).push(fn);
    },

    /** Register a slash command handler. ctx = { player, command, args }. */
    command(name, fn) {
      if (typeof fn !== "function") throw new Error("command(name, fn): fn must be a function");
      commands[String(name).toLowerCase()] = fn;
    },

    /** Broadcast a chat message to every player. */
    broadcast(message) {
      actions.push({ type: "broadcast", message: String(message) });
    },

    /** Send a private chat message to a single player by name. */
    tell(player, message) {
      actions.push({ type: "tell", player: String(player), message: String(message) });
    },

    /** Write a line to the server log. */
    log(message) {
      actions.push({ type: "log", message: String(message) });
    },

    /** Place a block by name (e.g. "stone") at world coordinates. */
    setBlock(x, y, z, block) {
      actions.push({ type: "set_block", x: x | 0, y: y | 0, z: z | 0, block: String(block) });
    },

    /** Kick a player by name with an optional reason. */
    kick(player, reason) {
      actions.push({ type: "kick", player: String(player), reason: String(reason || "Kicked by a mod") });
    },
  };

  function dispatch(event, data) {
    const list = handlers[event];
    if (!list) return;
    for (const fn of list) {
      try {
        fn(data);
      } catch (e) {
        actions.push({ type: "log", message: "[mod] handler error in '" + event + "': " + e });
      }
    }
  }

  function runCommand(ctx) {
    const fn = commands[String(ctx.command).toLowerCase()];
    if (!fn) {
      dispatch("command", ctx);
      return;
    }
    try {
      fn(ctx);
    } catch (e) {
      actions.push({ type: "log", message: "[mod] command error in '" + ctx.command + "': " + e });
    }
  }

  function drain() {
    const out = actions;
    actions = [];
    return JSON.stringify(out);
  }

  return { api, dispatch, runCommand, drain };
})();

globalThis.cubeplane = globalThis.__cubeplane.api;

// A tiny console shim so mods can use console.log for debugging.
globalThis.console = {
  log: function () {
    globalThis.cubeplane.log(Array.prototype.join.call(arguments, " "));
  },
};
globalThis.console.info = globalThis.console.log;
globalThis.console.warn = globalThis.console.log;
globalThis.console.error = globalThis.console.log;
