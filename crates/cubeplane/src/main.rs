//! cubeplane — a Minecraft server engine in Rust with a JavaScript (QuickJS)
//! mod runtime and an Atlas-powered admin panel.

use std::process::ExitCode;

use tracing_subscriber::{prelude::*, EnvFilter};

use cubeplane_server::Config;

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();

    // Allow an explicit config path as the first argument.
    let config_path = std::env::args().nth(1).unwrap_or_else(|| "cubeplane.toml".into());
    let config = match Config::load(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to load config {config_path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    print_banner(&config);

    // `run` handles Ctrl-C internally so it can flush a final save.
    if let Err(e) = cubeplane_server::run(config).await {
        tracing::error!("server error: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,cubeplane=info,cubeplane_server=info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();
}

fn print_banner(config: &Config) {
    println!(
        r#"
   ___      _           _
  / __\   _| |__   ___ | | __ _ _ __   ___
 / / | | | | '_ \ / _ \| |/ _` | '_ \ / _ \
/ /__| |_| | |_) |  __/| | (_| | | | |  __/
\____/\__,_|_.__/ \___||_|\__,_|_| |_|\___|
  Minecraft server engine · Rust + QuickJS mods

  bind:       {}:{}
  gamemode:   {}
  generator:  {}
  mods dir:   {}
  control:    {}
"#,
        config.server.host,
        config.server.port,
        config.server.gamemode,
        config.world.generator,
        if config.mods.enabled { &config.mods.dir } else { "(disabled)" },
        if config.control.enabled {
            format!("http://{}:{}", config.control.host, config.control.port)
        } else {
            "(disabled)".into()
        },
    );
}
