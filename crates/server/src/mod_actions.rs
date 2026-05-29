//! Applies [`ModAction`]s emitted by the JS runtime to the live server.

use std::sync::Arc;

use tokio::sync::mpsc::UnboundedReceiver;
use tracing::info;

use cubeplane_mods::ModAction;
use cubeplane_world::block;

use crate::clientbound as cb;
use crate::state::Shared;
use crate::text;

/// Consume mod actions until the runtime shuts the channel.
pub async fn run(shared: Arc<Shared>, mut rx: UnboundedReceiver<ModAction>) {
    while let Some(action) = rx.recv().await {
        apply(&shared, action);
    }
}

fn apply(shared: &Arc<Shared>, action: ModAction) {
    match action {
        ModAction::Broadcast { message } => {
            shared.broadcast(cb::system_chat(&text::plain(message), false));
        }
        ModAction::Tell { player, message } => {
            if let Some(p) = shared.player_by_name(&player) {
                p.send(cb::system_chat(&text::plain(message), false));
            }
        }
        ModAction::Log { message } => {
            info!(target: "cubeplane::mod", "{message}");
        }
        ModAction::SetBlock { x, y, z, block } => {
            if let Some(state) = block::by_name(&block) {
                {
                    let mut world = shared.world.lock().unwrap();
                    world.set_block(x, y, z, state);
                }
                shared.broadcast(cb::block_update(x, y, z, state));
            }
        }
        ModAction::Kick { player, reason } => {
            if let Some(p) = shared.player_by_name(&player) {
                p.send(cb::play_disconnect(&text::colored(reason, "red")));
            }
        }
        ModAction::Unknown => {}
    }
}
