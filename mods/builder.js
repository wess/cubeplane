// builder.js — example build commands that place blocks via the engine.
//
// Demonstrates `cubeplane.command(name, fn)` and `cubeplane.setBlock(...)`.
// Command context is { player, command, args } where args is a string array.

// /tower [height] — raises a stone pillar near world spawn.
cubeplane.command("tower", (ctx) => {
  const height = Math.max(1, Math.min(64, parseInt(ctx.args[0] || "10", 10)));
  const baseX = 0, baseZ = 0, baseY = 4;
  for (let i = 0; i < height; i++) {
    cubeplane.setBlock(baseX, baseY + i, baseZ, i % 2 === 0 ? "stone" : "cobblestone");
  }
  cubeplane.setBlock(baseX, baseY + height, baseZ, "glass");
  cubeplane.tell(ctx.player, "Built a " + height + "-block tower at 0," + baseY + ",0");
});

// /platform <size> — lays a square glass platform at y=5 centered on spawn.
cubeplane.command("platform", (ctx) => {
  const size = Math.max(1, Math.min(32, parseInt(ctx.args[0] || "8", 10)));
  const half = Math.floor(size / 2);
  for (let x = -half; x <= half; x++) {
    for (let z = -half; z <= half; z++) {
      cubeplane.setBlock(x, 5, z, "glass");
    }
  }
  cubeplane.tell(ctx.player, "Laid a " + size + "x" + size + " glass platform.");
});
