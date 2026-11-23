//! Integrations: external Craft CLIs / MCP servers and Blender, installed from
//! Settings → Integrations (`GET`/`POST /integrations`). A catalog entry is a
//! git repo built with cargo into `<data>/integrations/<id>/`, or a flatpak.
//! Whatever has an MCP entry point is written to
//! `<data>/integrations/mcp.json` as `metal_agent::mcp::McpServerConfig`
//! shaped rows, so the agent can attach it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

use crate::call::{CallError, Ctx, Params};

enum Source {
    Cargo { repo: &'static str, package: &'static str },
    Flatpak { app: &'static str },
}

struct Entry {
    id: &'static str,
    name: &'static str,
    what: &'static str,
    source: Source,
    /// Built binary name (cargo) or launcher command (flatpak).
    bin: &'static str,
    /// Arguments that start the stdio MCP server; `None` = no MCP yet.
    mcp: Option<&'static [&'static str]>,
}

const CATALOG: &[Entry] = &[
    Entry {
        id: "photocraft",
        name: "PhotoCraft",
        what: "logos and advanced text to PNG (CLI + MCP)",
        source: Source::Cargo { repo: "https://github.com/storytold/photocraft.git", package: "photocraft-cli" },
        bin: "photocraft-cli",
        mcp: Some(&["mcp", "--headless"]),
    },
    Entry {
        id: "effectcraft",
        name: "EffectCraft",
        what: "animated logos and text animation (CLI + MCP)",
        source: Source::Cargo { repo: "https://github.com/storytold/effectcraft.git", package: "effectcraft-cli" },
        bin: "effectcraft-cli",
        mcp: Some(&["mcp"]),
    },
    Entry {
        id: "vectorcraft",
        name: "VectorCraft",
        what: "vector art, the MCP pattern the others follow (CLI + MCP)",
        source: Source::Cargo { repo: "https://github.com/storytold/vectorcraft.git", package: "vectorcraft-cli" },
        bin: "vectorcraft-cli",
        mcp: Some(&["mcp", "--headless"]),
    },
    Entry {
        id: "blender",
        name: "Blender",
        what: "modelling objects (flatpak, headless -b)",
        source: Source::Flatpak { app: "org.blender.Blender" },
        bin: "flatpak",
        mcp: None,
    },
];

#[derive(Clone, Default)]
struct Job {
    state: &'static str, // "running" | "done" | "failed"
    log: String,
}

fn jobs() -> &'static Mutex<HashMap<&'static str, Job>> {
    static J: OnceLock<Mutex<HashMap<&'static str, Job>>> = OnceLock::new();
    J.get_or_init(Default::default)
}

fn root() -> PathBuf {
    metal_perform::output::data_root().join("integrations")
}

fn bin_path(e: &Entry) -> Option<PathBuf> {
    match e.source {
        Source::Cargo { .. } => {
            let p = root().join(e.id).join("target/release").join(e.bin);
            p.is_file().then_some(p)
        }
        Source::Flatpak { app } => Command::new("flatpak")
            .args(["info", app])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|_| PathBuf::from("flatpak")),
    }
}

fn row(e: &Entry) -> Value {
    let job = jobs().lock().ok().and_then(|j| j.get(e.id).cloned());
    let bin = bin_path(e);
    json!({
        "id": e.id, "name": e.name, "what": e.what,
        "installed": bin.is_some(),
        "bin": bin.map(|p| p.display().to_string()),
        "mcp": e.mcp.is_some(),
        "job": job.as_ref().map(|j| j.state),
        "log": job.map(|j| j.log.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")),
    })
}

/// Rewrites `mcp.json` from what is installed.
fn write_mcp() {
    let servers: Vec<Value> = CATALOG
        .iter()
        .filter_map(|e| {
            let args = e.mcp?;
            let bin = bin_path(e)?;
            Some(json!({ "name": e.id, "transport": {
                "kind": "stdio", "command": bin.display().to_string(), "args": args } }))
        })
        .collect();
    let _ = std::fs::create_dir_all(root());
    let _ = std::fs::write(root().join("mcp.json"), serde_json::to_vec_pretty(&servers).unwrap_or_default());
}

fn run_step(id: &'static str, cmd: &mut Command) -> bool {
    let out = cmd.output();
    let (ok, text) = match out {
        Ok(o) => (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))),
        Err(e) => (false, e.to_string()),
    };
    if let Ok(mut j) = jobs().lock() {
        let job = j.entry(id).or_default();
        job.log.push_str(&text);
    }
    ok
}

fn install(e: &'static Entry) {
    if let Ok(mut j) = jobs().lock() {
        if j.get(e.id).is_some_and(|j| j.state == "running") {
            return;
        }
        j.insert(e.id, Job { state: "running", log: String::new() });
    }
    std::thread::spawn(move || {
        let ok = match &e.source {
            Source::Cargo { repo, package } => {
                let dir = root().join(e.id);
                let _ = std::fs::create_dir_all(root());
                let fetch = if dir.join(".git").is_dir() {
                    run_step(e.id, Command::new("git").arg("-C").arg(&dir).args(["pull", "--ff-only"]))
                } else {
                    run_step(e.id, Command::new("git").args(["clone", "--depth", "1", repo]).arg(&dir))
                };
                fetch && run_step(e.id, Command::new("cargo").current_dir(&dir).args(["build", "--release", "-p", package]))
            }
            Source::Flatpak { app } => run_step(e.id, Command::new("flatpak").args(["install", "--user", "-y", "flathub", app])),
        };
        if let Ok(mut j) = jobs().lock() {
            j.entry(e.id).or_default().state = if ok { "done" } else { "failed" };
        }
        write_mcp();
    });
}

/// `GET /integrations`: the catalog with install state and the last job log.
pub fn get(_ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    Ok(json!({ "integrations": CATALOG.iter().map(row).collect::<Vec<_>>(), "mcp_config": root().join("mcp.json") }))
}

/// `POST /integrations {id, op: "install"|"update"|"remove"}`.
pub fn post(_ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let body = params.json().ok_or_else(|| CallError::Bad("body must be JSON {id, op}".into()))?;
    let id = body["id"].as_str().unwrap_or("");
    let e = CATALOG.iter().find(|e| e.id == id).ok_or_else(|| CallError::Bad(format!("unknown integration {id:?}")))?;
    match body["op"].as_str().unwrap_or("install") {
        "install" | "update" => install(e),
        "remove" => match e.source {
            Source::Cargo { .. } => {
                let _ = std::fs::remove_dir_all(root().join(e.id));
                if let Ok(mut j) = jobs().lock() {
                    j.remove(e.id);
                }
                write_mcp();
            }
            Source::Flatpak { .. } => return Err(CallError::Bad("remove Blender with flatpak uninstall".into())),
        },
        other => return Err(CallError::Bad(format!("unknown op {other:?}"))),
    }
    Ok(row(e))
}
