//! One sync node, run in-process or from `metal-sync`'s `main`.
//!
//! `main.rs` is CLI wiring; the node itself lives here so the Android shell
//! can start the same engine on a background thread (`metal-dash`'s
//! `android_main`) with its files dir as the data root and the control
//! surface on `127.0.0.1:7901`. Nothing in here reads the environment or the
//! process's argv: every path is passed in.

use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};

use crate::{
    config,
    engine::{EngineOpts, Join, SyncEngine},
    http, identity, SyncState,
};

/// Where a node keeps what it needs. `config_dir` holds `sync.toml` and
/// `config.toml`; `key` is the node's secret; `store` is iroh's own databases,
/// never inside `data` (that is what gets synced).
#[derive(Clone, Debug)]
pub struct NodePaths {
    pub data: PathBuf,
    pub key: PathBuf,
    /// `sync.toml` itself (its parent is the config dir).
    pub sync: PathBuf,
    pub config_dir: PathBuf,
    pub store: PathBuf,
}

/// What the node does once it is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Serve the control surface and sync until the process ends.
    Run,
    /// Print the pairing ticket and return.
    Pair,
}

#[derive(Clone, Debug)]
pub struct NodeOpts {
    pub paths: NodePaths,
    /// A ticket to join, which wins over the namespace `sync.toml` remembers.
    pub ticket: Option<String>,
    pub mode: Mode,
    /// Control surface address. Loopback: this is for the operator and the
    /// local metal-dash, never for peers.
    pub control_host: String,
    pub control_port: u16,
    /// The local metal-dash that landed files are announced to.
    pub dash_port: u16,
}

impl NodeOpts {
    /// The defaults `metal-sync` has always used, from the XDG config dir.
    pub fn desktop(paths: NodePaths) -> Self {
        NodeOpts {
            paths,
            ticket: None,
            mode: Mode::Run,
            control_host: "127.0.0.1".to_string(),
            control_port: 7901,
            dash_port: 7900,
        }
    }
}

/// The control port `metal-sync` binds when `--port` is not given: the
/// `FW_SYNC_PORT` value when it is a port from 1 to 65535, else 7901. The same
/// rule `metal-dash` uses to find `metal-sync`, so the two agree on one variable.
pub fn control_port_from(value: Option<&str>) -> u16 {
    value
        .and_then(|v| v.trim().parse::<u16>().ok())
        .filter(|p| *p != 0)
        .unwrap_or(7901)
}

#[cfg(test)]
mod port_tests {
    use super::control_port_from;

    #[test]
    fn unset_or_unusable_falls_back_to_7901() {
        assert_eq!(control_port_from(None), 7901);
        assert_eq!(control_port_from(Some("")), 7901);
        assert_eq!(control_port_from(Some("abc")), 7901);
        assert_eq!(control_port_from(Some("0")), 7901);
        assert_eq!(control_port_from(Some("70000")), 7901);
    }

    #[test]
    fn env_port_is_read_and_trimmed() {
        assert_eq!(control_port_from(Some("7911")), 7911);
        assert_eq!(control_port_from(Some(" 7912 ")), 7912);
    }
}

/// The default paths on a desktop: `~/.config/metalvj/…`.
pub fn desktop_paths(data: PathBuf) -> NodePaths {
    let config_dir = identity::config_dir();
    NodePaths {
        data,
        key: identity::default_path(),
        sync: config::default_path(),
        store: config_dir.join("store"),
        config_dir,
    }
}

/// Run one node to completion (or to the pairing ticket).
pub async fn run(opts: NodeOpts) -> Result<()> {
    let paths = &opts.paths;
    let config_path = paths.sync.clone();
    std::fs::create_dir_all(&paths.config_dir)
        .with_context(|| format!("creating {}", paths.config_dir.display()))?;
    let secret = identity::load_or_create(&paths.key).map_err(anyhow::Error::msg)?;
    let node_id = secret.public().to_string();

    let cfg = config::load(&config_path).map_err(anyhow::Error::msg)?;

    let join = match &opts.ticket {
        Some(ticket) => Join::Ticket(
            iroh_docs::DocTicket::from_str(ticket).context("that ticket did not parse")?,
        ),
        None => match &cfg.namespace {
            Some(hex) => Join::Existing(
                iroh_docs::NamespaceId::from_str(hex)
                    .with_context(|| format!("sync.toml has an unusable namespace {hex:?}"))?,
            ),
            None => Join::Create,
        },
    };

    // `[sync].allowseed` is read from the `config.toml` next to `sync.toml`
    // on every scan (see `SyncState::allowlist`).
    let config_toml = metal_config::default_path(&paths.config_dir);
    let state = Arc::new(Mutex::new(
        SyncState::new(node_id.clone(), paths.data.clone()).with_config(config_toml),
    ));
    let config_toml_for_farm = metal_config::default_path(&paths.config_dir);
    let engine_opts = EngineOpts {
        name: cfg.name.clone(),
        peers: cfg.peers.clone(),
        config_path: Some(config_path.clone()),
        dash_port: opts.dash_port,
        mdns: true,
        local_only: false,
        hold_joiner: true,
        hold_timeout: cfg
            .join_hold_secs
            .map(std::time::Duration::from_secs)
            .unwrap_or(crate::engine::DEFAULT_HOLD_TIMEOUT),
        // `[farm]` in the central config.toml (conductor, shard size, beats).
        farm: metal_config::Config::load(&config_toml_for_farm)
            .map(|c| crate::farm_net::FarmCfg::from(&c.farm))
            .unwrap_or_default(),
    };
    let (engine, net) = SyncEngine::spawn_with(
        secret,
        paths.data.clone(),
        paths.store.clone(),
        join,
        Arc::clone(&state),
        engine_opts,
    )
    .await?;
    let engine = engine.with_dash("127.0.0.1".to_string(), opts.dash_port);

    // Remember which document we ended up in, so the next start rejoins it
    // rather than creating a stranger.
    let namespace = engine.namespace().to_string();
    if cfg.namespace.as_deref() != Some(namespace.as_str()) {
        config::save(
            &config_path,
            &config::SyncConfig { namespace: Some(namespace.clone()), ..cfg.clone() },
        )
        .map_err(anyhow::Error::msg)?;
    }

    if opts.mode == Mode::Pair {
        println!("{}", engine.ticket().await?);
        return Ok(());
    }

    println!("metal-sync {node_id}");
    println!("document {namespace}");

    // Mint the ticket now, while the endpoint is bound, and serve it from
    // `GET /ticket`. See `SyncState::ticket` for why it cannot come from a
    // separate short-lived process.
    match engine.ticket().await {
        Ok(t) => state.lock().expect("poisoned").set_ticket(t.to_string()),
        Err(e) => eprintln!("metal-sync: could not mint a pairing ticket: {e}"),
    }

    // Catch up on what the document already holds before going live.
    match engine.land_existing().await {
        Ok(n) if n > 0 => println!("landed {n} files from the document"),
        Ok(_) => {}
        Err(e) => eprintln!("metal-sync: catch-up failed: {e}"),
    }

    let ctl = http::Control {
        state: Arc::clone(&state),
        net: Some((net, tokio::runtime::Handle::current())),
        dash_port: opts.dash_port,
    };
    serve_control(&opts.control_host, opts.control_port, ctl)?;
    engine.run().await
}

/// The blocking localhost control surface, on its own threads.
///
/// It does not belong on the tokio runtime: it serves a handful of JSON
/// requests, and the runtime this process exists to host belongs to iroh.
pub fn serve_control(host: &str, port: u16, ctl: http::Control) -> Result<()> {
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .with_context(|| format!("invalid address {host}:{port}"))?;
    let listener = TcpListener::bind(addr).with_context(|| format!("binding {addr}"))?;
    println!("control on {addr}");

    std::thread::spawn(move || {
        for incoming in listener.incoming() {
            let Ok(stream) = incoming else { continue };
            let ctl = ctl.clone();
            std::thread::spawn(move || {
                if let Err(e) = http::handle(stream, &ctl) {
                    eprintln!("metal-sync: connection error: {e}");
                }
            });
        }
    });
    Ok(())
}

/// [`run`] on a thread of its own, with its own multi-thread tokio runtime.
///
/// For a host that has no runtime of its own (the Android shell's
/// `android_main`): the node runs beside the window and the shell never waits
/// on it. The thread ends when the node does; its error goes to stderr.
pub fn spawn_thread(opts: NodeOpts) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("metal-sync".into()).spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("metal-sync: no runtime: {e}");
                return;
            }
        };
        if let Err(e) = runtime.block_on(run(opts)) {
            eprintln!("metal-sync: {e:#}");
        }
    })
}
