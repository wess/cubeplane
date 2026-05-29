// welcome.js — greets players and announces comings & goings.
//
// Mods are plain JavaScript evaluated by the embedded QuickJS runtime. They
// register handlers with `cubeplane.on(event, fn)` and act on the world through
// the `cubeplane` API. See docs/MODDING.md for the full reference.

cubeplane.on("server_start", (e) => {
  cubeplane.log("welcome.js loaded — cubeplane " + e.version);
});

cubeplane.on("player_join", (e) => {
  // Greet everyone, then whisper a tip to the new arrival.
  cubeplane.broadcast("§a" + e.player + " has entered the cubeplane!");
  cubeplane.tell(e.player, "Welcome, " + e.player + "! Type /help for commands.");
});

cubeplane.on("player_leave", (e) => {
  cubeplane.broadcast("§7" + e.player + " drifted away…");
});
