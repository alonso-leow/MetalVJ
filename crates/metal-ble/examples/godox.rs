//! `godox` — a small command-line front end for the Godox Bluetooth Mesh link.
//!
//! Usage:
//!
//! ```text
//!   godox scan [--seconds N] [--json]
//!   godox provision --mesh mesh.json [--net-key HEX] [--unicast N] [--mac ADDR]
//!   godox import <ha-godox-mesh mesh_state.json> --mesh mesh.json
//!   godox set --mesh mesh.json --level 0.5 [--cct 4300] [--hue 200 --sat 1] [--mac ADDR]
//!   godox ramp --mesh mesh.json --seconds 5 [--mac ADDR]
//! ```
//!
//! Plain `std::env::args` parsing — the plan asks for no new dependency for a
//! tool this small, and every option here is a single value.
//!
//! This is the Phase-A CLI. It talks to real hardware; the hardware path is
//! **untested** (Phase D owns that), so treat a failure here as "not yet tried
//! on a light", not as proof the protocol is wrong — the protocol itself is
//! covered by the crate's spec-vector tests.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use metal_ble::state::MeshState;
use metal_ble::{godox, LightState, Protocol};

fn usage() -> ! {
    eprintln!(
        "usage:\n  \
         godox scan [--seconds N] [--json]\n  \
         godox provision --mesh mesh.json [--net-key HEX] [--unicast N] [--mac ADDR]\n  \
         godox import <source.json> --mesh mesh.json\n  \
         godox set --mesh mesh.json --level 0.5 [--cct 4300] [--hue 200 --sat 1] [--mac ADDR]\n  \
         godox ramp --mesh mesh.json --seconds 5 [--mac ADDR]"
    );
    std::process::exit(2)
}

/// Minimal `--flag value` argument reader.
struct Args {
    positional: Vec<String>,
    flags: std::collections::HashMap<String, String>,
}

impl Args {
    fn parse() -> Args {
        let mut positional = Vec::new();
        let mut flags = std::collections::HashMap::new();
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            if let Some(name) = arg.strip_prefix("--") {
                // `--json` is a boolean switch; everything else takes a value.
                if name == "json" {
                    flags.insert(name.to_string(), "true".into());
                } else if let Some((k, v)) = name.split_once('=') {
                    flags.insert(k.to_string(), v.to_string());
                } else if let Some(value) = it.next() {
                    flags.insert(name.to_string(), value);
                } else {
                    eprintln!("flag --{name} needs a value");
                    usage();
                }
            } else {
                positional.push(arg);
            }
        }
        Args { positional, flags }
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.flags.get(name).map(|s| s.as_str())
    }

    fn get_f32(&self, name: &str) -> Option<f32> {
        self.get(name).and_then(|v| v.parse().ok())
    }

    fn get_u64(&self, name: &str) -> Option<u64> {
        self.get(name).and_then(|v| v.parse().ok())
    }

    fn get_u16(&self, name: &str) -> Option<u16> {
        self.get(name).and_then(|v| v.parse().ok())
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

fn main() {
    let args = Args::parse();
    let Some(command) = args.positional.first().cloned() else {
        usage();
    };
    let result = match command.as_str() {
        "scan" => cmd_scan(&args),
        "provision" => cmd_provision(&args),
        "import" => cmd_import(&args),
        "set" => cmd_set(&args),
        "ramp" => cmd_ramp(&args),
        other => {
            eprintln!("unknown command {other}");
            usage();
        }
    };
    if let Err(message) = result {
        eprintln!("error: {message}");
        std::process::exit(1);
    }
}

fn cmd_scan(args: &Args) -> Result<(), String> {
    let seconds = args.get_u64("seconds").unwrap_or(8);
    let json = args.get("json").is_some();
    let rt = runtime();
    let devices = rt.block_on(metal_ble::transport::scan(seconds))?;
    if json {
        for d in &devices {
            println!(
                "{{\"address\":\"{}\",\"name\":\"{}\",\"kind\":\"{}\",\"services\":[{}]}}",
                d.address,
                d.name.replace('"', "\\\""),
                d.kind(),
                d.services
                    .iter()
                    .map(|u| format!("\"{u}\""))
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
    } else {
        println!("{:<20} {:<18} {:<14} services", "address", "name", "kind");
        for d in &devices {
            println!(
                "{:<20} {:<18} {:<14} {}",
                d.address,
                if d.name.is_empty() { "-" } else { &d.name },
                d.kind(),
                d.services
                    .iter()
                    .map(|u| {
                        // SIG base UUIDs print as their 16-bit short form: the
                        // low 96 bits are the fixed Bluetooth suffix.
                        let v = u.as_u128();
                        let suffix = v & 0x0000_0000_0000_ffff_ffff_ffff_ffff_u128;
                        let short = (v >> 96) as u16;
                        if short != 0 && suffix == 0x0000_1000_8000_0080_5f9b_34fb_u128 {
                            format!("{short:#06x}")
                        } else {
                            format!("{u}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        if devices.is_empty() {
            println!("(no devices seen in {seconds}s)");
        }
    }
    Ok(())
}

fn cmd_provision(args: &Args) -> Result<(), String> {
    let mesh_path = PathBuf::from(args.get("mesh").ok_or("--mesh is required")?);
    let mac = args.get("mac").unwrap_or("").to_string();
    let unicast = args.get_u16("unicast").unwrap_or(2);
    let net_key = match args.get("net-key") {
        Some(hex) => parse_key16(hex)?,
        None => random_key()?,
    };
    let app_key = random_key()?;
    let iv_index = 0u32;

    let rt = runtime();
    let mut bearer = rt.block_on(metal_ble::transport::ProvisioningGatt::connect(&mac))?;
    println!("connected to provisioning bearer at {}", bearer.address());

    let mut session = metal_ble::provision::ProvisioningSession::new(net_key, 0, iv_index, unicast);
    let provisioned = rt.block_on(session.run(&mut bearer))?;
    rt.block_on(bearer.disconnect());
    println!(
        "provisioned node at {:#06x} ({} element(s))",
        provisioned.unicast_address, provisioned.num_elements
    );

    // Persist the result before doing anything else.
    let mut state = MeshState {
        network_key: hex::encode(provisioned.net_key),
        app_key: hex::encode(app_key),
        provisioner_address: 1,
        node_address: provisioned.unicast_address,
        sequence_number: 0,
        iv_index: provisioned.iv_index,
        device_key: hex::encode(provisioned.device_key),
        device_address: bearer.address(),
        num_elements: provisioned.num_elements,
        nodes: vec![],
        next: 0,
    };
    state
        .save(&mesh_path)
        .map_err(|e| format!("saving mesh state: {e}"))?;
    println!("saved {}", mesh_path.display());

    // Now bind the app key over the proxy (device-key path).
    let proxy = rt.block_on(metal_ble::transport::MeshProxy::connect(&state.device_address))?;
    let config = metal_ble::transport::ConfigSession::new(&proxy, &mut state, &mesh_path);
    let mut config = config;
    rt.block_on(config.run())?;
    rt.block_on(proxy.disconnect());
    println!("app key installed and bound to the vendor model");
    Ok(())
}

fn cmd_import(args: &Args) -> Result<(), String> {
    let source = args
        .positional
        .get(1)
        .ok_or("import needs a source mesh_state.json path")?;
    let mesh_path = PathBuf::from(args.get("mesh").ok_or("--mesh is required")?);
    // `MeshState::load` accepts the ha-godox-mesh flat shape directly.
    let state = MeshState::load(source).map_err(|e| format!("reading {source}: {e}"))?;
    state
        .save(&mesh_path)
        .map_err(|e| format!("saving {}: {e}", mesh_path.display()))?;
    println!(
        "imported mesh state from {source} to {} (node {:#06x}, seq {})",
        mesh_path.display(),
        state.node_address,
        state.sequence_number
    );
    Ok(())
}

fn cmd_set(args: &Args) -> Result<(), String> {
    let mesh_path = PathBuf::from(args.get("mesh").ok_or("--mesh is required")?);
    let mac = args.get("mac").unwrap_or("").to_string();
    let mut state = MeshState::load(&mesh_path).map_err(|e| format!("reading mesh: {e}"))?;

    let level = args.get_f32("level").unwrap_or(0.0);
    let cct = args.get_f32("cct");
    let hue = args.get_f32("hue");
    let sat = args.get_f32("sat");
    let hsi = match (hue, sat) {
        (Some(hue), Some(sat)) => Some(metal_ble::Hsi { hue_deg: hue, sat }),
        _ => None,
    };
    let light = LightState { level, cct_kelvin: cct, hsi };

    let dst = state.destination(args.get_u16("node"));
    let seq = state.next_seq(&mesh_path).map_err(|e| format!("reserving seq: {e}"))?;
    let access = godox::encode(&light);
    let pdu = metal_ble::transport::pack_command(&access, &state, dst, 10, seq);

    let rt = runtime();
    let proxy = rt.block_on(metal_ble::transport::MeshProxy::connect(&mac))?;
    rt.block_on(proxy.write(&pdu))?;
    rt.block_on(proxy.disconnect());
    println!(
        "sent {} bytes to {:#06x} (seq {seq}, level {level}, cct {:?}, hsi {:?})",
        pdu.len(),
        dst,
        cct,
        hsi.map(|h| (h.hue_deg, h.sat))
    );
    Ok(())
}

fn cmd_ramp(args: &Args) -> Result<(), String> {
    let mesh_path = PathBuf::from(args.get("mesh").ok_or("--mesh is required")?);
    let mac = args.get("mac").unwrap_or("").to_string();
    let seconds = args.get_u64("seconds").unwrap_or(5).max(1);
    let mut state = MeshState::load(&mesh_path).map_err(|e| format!("reading mesh: {e}"))?;
    let dst = state.destination(args.get_u16("node"));

    let rt = runtime();
    let proxy = rt.block_on(metal_ble::transport::MeshProxy::connect(&mac))?;

    // 0 → 1 → 0, resending at 25 Hz so the ramp is visible as float authoring.
    let steps = seconds * 25;
    let start = Instant::now();
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let level = if t <= 0.5 { t * 2.0 } else { (1.0 - t) * 2.0 };
        let seq = state.next_seq(&mesh_path).map_err(|e| format!("reserving seq: {e}"))?;
        let access = godox::encode(&LightState {
            level,
            cct_kelvin: None,
            hsi: None,
        });
        let pdu = metal_ble::transport::pack_command(&access, &state, dst, 10, seq);
        rt.block_on(proxy.write(&pdu))?;
        // Pace at 25 Hz even if a write took no time.
        let target = Duration::from_millis(i * 40);
        let elapsed = start.elapsed();
        if target > elapsed {
            std::thread::sleep(target - elapsed);
        }
    }
    rt.block_on(proxy.disconnect());
    println!("ramped 0→1→0 over {seconds}s to {dst:#06x}");
    Ok(())
}

fn parse_key16(hex_str: &str) -> Result<[u8; 16], String> {
    let bytes = hex::decode(hex_str).map_err(|e| format!("bad hex key: {e}"))?;
    bytes
        .try_into()
        .map_err(|_| format!("key must be 16 bytes (32 hex chars), got {}", hex_str.len()))
}

fn random_key() -> Result<[u8; 16], String> {
    let mut key = [0u8; 16];
    getrandom::getrandom(&mut key).map_err(|e| format!("rng: {e}"))?;
    Ok(key)
}

// Keep the protocol enum referenced so the example fails to build if the API
// it targets changes shape.
#[allow(dead_code)]
fn _protocol_is_godox_mesh() -> Protocol {
    Protocol::GodoxMesh
}
