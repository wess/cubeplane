use std::time::Duration;

use cubeplane_mods::{ModAction, ModEvent, ModRuntime};

/// Write a couple of mods to a temp dir, fire events, and assert the resulting
/// actions come back across the QuickJS boundary.
#[tokio::test]
async fn dispatches_events_and_commands() {
    let dir = std::env::temp_dir().join(format!("cubeplane-mods-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("greeter.js"),
        r#"
        cubeplane.on("player_join", (e) => {
            cubeplane.broadcast(e.player + " joined");
        });
        cubeplane.command("ping", (ctx) => {
            cubeplane.tell(ctx.player, "pong");
        });
        "#,
    )
    .unwrap();

    let (rt, mut actions) = ModRuntime::spawn(&dir);
    assert_eq!(rt.loaded().len(), 1);

    rt.fire(ModEvent::PlayerJoin {
        player: "wess".into(),
        uuid: "00000000-0000-0000-0000-000000000000".into(),
        entity_id: 1,
    });

    let action = tokio::time::timeout(Duration::from_secs(5), actions.recv())
        .await
        .expect("timed out")
        .expect("channel closed");
    match action {
        ModAction::Broadcast { message } => assert_eq!(message, "wess joined"),
        other => panic!("unexpected action: {other:?}"),
    }

    rt.fire(ModEvent::Command {
        player: "wess".into(),
        command: "ping".into(),
        args: vec![],
    });
    let action = tokio::time::timeout(Duration::from_secs(5), actions.recv())
        .await
        .expect("timed out")
        .expect("channel closed");
    match action {
        ModAction::Tell { player, message } => {
            assert_eq!(player, "wess");
            assert_eq!(message, "pong");
        }
        other => panic!("unexpected action: {other:?}"),
    }

    rt.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
