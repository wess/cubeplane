//! # cubeplane-mods
//!
//! A QuickJS-powered JavaScript mod runtime for cubeplane, via [`rquickjs`].
//!
//! Mods are plain `.js` files dropped in a directory. They register handlers
//! against engine events and respond by enqueuing actions:
//!
//! ```js
//! cubeplane.on("player_join", (e) => {
//!   cubeplane.broadcast(e.player + " joined the cubeplane!");
//! });
//!
//! cubeplane.command("spawn", (ctx) => {
//!   cubeplane.tell(ctx.player, "Teleporting you home…");
//! });
//! ```
//!
//! The runtime runs on its own thread ([`ModRuntime`]); the engine fires
//! [`ModEvent`]s in and applies the [`ModAction`]s that come back.

mod event;
mod host;

pub use event::{ModAction, ModEvent};
pub use host::ModRuntime;
