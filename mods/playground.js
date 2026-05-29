// playground.js — small reactive behaviours and a periodic announcer.
//
// Shows the "tick" event (fired once per second), the "chat" event, and the
// block events ("block_place" / "block_break").

let seconds = 0;

cubeplane.on("tick", (e) => {
  seconds = e.tick;
  // Every five minutes, drop a friendly reminder.
  if (seconds > 0 && seconds % 300 === 0) {
    cubeplane.broadcast("§b[cubeplane] Server has been up for " + (seconds / 60) + " minutes.");
  }
});

// React to keywords in chat.
cubeplane.on("chat", (e) => {
  const msg = e.message.toLowerCase();
  if (msg.includes("hello") || msg.includes("hi")) {
    cubeplane.tell(e.player, "§eThe cubeplane greets you back, " + e.player + "!");
  }
});

// Confirm builds and breaks.
cubeplane.on("block_place", (e) => {
  if (e.block === "oak_log") {
    cubeplane.tell(e.player, "§2Nice, planting trees I see.");
  }
});

cubeplane.on("block_break", (e) => {
  cubeplane.log(e.player + " broke a block at " + e.x + "," + e.y + "," + e.z);
});

// /uptime — report how long the server has been running.
cubeplane.command("uptime", (ctx) => {
  cubeplane.tell(ctx.player, "§aUptime: " + seconds + "s");
});
