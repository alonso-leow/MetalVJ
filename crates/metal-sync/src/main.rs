//! `metal-sync` — the isolated async process.
//!
//! Everything here is CLI and process wiring; the work lives in the library
//! (`metal_sync::*`) so it can be tested without a socket or a network. See
//! `lib.rs` for why this is a separate process at all.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use metal_sync::{
    config,
    identity,
    node::{self, Mode, NodeOpts, NodePaths},
};

#[derive(Parser)]
#[command(name = "metal-sync", about = "P2P data sync for MetalVJ")]
struct Cli {
    /// Local control port. Defaults to `FW_SYNC_PORT`, else 7901. Distinct
    /// from metal-dash's 7900.
    #[arg(long)]
    port: Option<u16>,

    /// Loopback by default: this surface is for the operator and for metal-dash,
    /// never for peers. Peers arrive over QUIC, not here.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Where `data/` lives. Must be the same tree metal-dash was given.
    #[arg(long)]
    data: Option<PathBuf>,

    /// Where this node's secret key lives.
    #[arg(long)]
    key: Option<PathBuf>,

    /// Where `sync.toml` lives.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Port of the local metal-dash to notify after landing files it caches.
    #[arg(long, default_value_t = 7900)]
    dash_port: u16,

    /// Where iroh's own blob and doc databases live. Never inside `data/` —
    /// `data/` is what gets synced, and a store that synced itself is a loop.
    #[arg(long)]
    store: Option<PathBuf>,

    /// Validate `config.toml` (migrating from `sync.toml`/`ai.json` first if
    /// neither has one yet), print every lint and what this run would
    /// actually do, and exit — no engine, no network. See `metal_sync::doctor`.
    #[arg(long)]
    doctor: bool,

    /// Join the document this ticket points at, then keep running.
    ///
    /// Joining has to happen inside the process that goes on to sync: the
    /// ticket names peer addresses, and importing it in a short-lived process
    /// that then exits means nothing is left connected to anybody.
    #[arg(long)]
    ticket: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the sync process. The default when no subcommand is given.
    Run,
    /// Print this node's id.
    Id,
    /// Print a ticket another device can join with.
    Pair,
    /// Join the document a ticket points at, then run. Same as `--ticket`.
    Join {
        /// The ticket served by `GET /ticket` on the other device.
        ticket: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Off unless asked for. `FW_SYNC_LOG=info` is how you watch a sync happen,
    // and `FW_SYNC_LOG=debug` is how you find out why one did not.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("FW_SYNC_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    let data_root = cli.data.clone().unwrap_or_else(|| PathBuf::from("data"));
    let config_path = cli.config.clone().unwrap_or_else(config::default_path);
    let store_root =
        cli.store.clone().unwrap_or_else(|| identity::config_dir().join("store"));

    if cli.doctor {
        let config_dir = config_path.parent().map(PathBuf::from).unwrap_or_else(identity::config_dir);
        for line in metal_sync::doctor::run(&config_dir, &data_root) {
            println!("{line}");
        }
        return Ok(());
    }

    let key_path = cli.key.clone().unwrap_or_else(identity::default_path);
    if matches!(cli.command, Some(Command::Id)) {
        let secret = identity::load_or_create(&key_path).map_err(anyhow::Error::msg)?;
        println!("{}", secret.public());
        return Ok(());
    }

    // A ticket on the command line wins over whatever `sync.toml` remembers —
    // joining is an explicit act, and it is how a device moves between shows.
    let ticket = match &cli.command {
        Some(Command::Join { ticket }) => Some(ticket.clone()),
        _ => cli.ticket.clone(),
    };
    let mode = match cli.command {
        Some(Command::Pair) => Mode::Pair,
        _ => Mode::Run,
    };
    let config_dir = config_path.parent().map(PathBuf::from).unwrap_or_else(identity::config_dir);
    let opts = NodeOpts {
        paths: NodePaths { data: data_root, key: key_path, sync: config_path, config_dir, store: store_root },
        ticket,
        mode,
        control_host: cli.host.clone(),
        control_port: cli.port.unwrap_or_else(|| {
            node::control_port_from(std::env::var("FW_SYNC_PORT").ok().as_deref())
        }),
        dash_port: cli.dash_port,
    };
    node::run(opts).await
}
