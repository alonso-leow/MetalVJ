//! The radio: btleplug scan, connect and GATT Proxy I/O.
//!
//! ### Why btleplug and not `bluer`
//!
//! `bluer` is Linux/BlueZ only. The plan wants one link crate that a show can
//! run on the machines the project already targets — Linux (BlueZ), macOS
//! (CoreBluetooth) and Windows (WinRT) — so the transport is `btleplug`, which
//! is a single cross-platform facade over exactly those three backends. The
//! cost is a less direct mapping to BlueZ semantics; the benefit is that the
//! Godox link is not Linux-only.
//!
//! ### What a Godox light exposes
//!
//! * Unprovisioned nodes advertise the **Mesh Provisioning Service** `0x1827`
//!   (Data In `0x2ADB`, Data Out `0x2ADC`).
//! * Provisioned nodes advertise the **Mesh Proxy Service** `0x1828`
//!   (Data In `0x2ADD`, Data Out `0x2ADE`). All runtime commands go here.
//!
//! Data Out is a notify characteristic; Data In is written **without response**
//! (the reference does the same — a slow acknowledged write would cap the
//! command rate far below the 20–30 Hz the plan asks for).
//!
//! ### What is *not* here
//!
//! This module performs I/O and nothing else: it does not encrypt, does not
//! rate-limit and does not reconnect. [`crate::link`] owns those policies and
//! [`crate::transport::ConfigSession`] owns the post-provisioning AppKey bind.
//! Keeping the split sharp is what makes the protocol testable without a radio.

use btleplug::api::{
    Central, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::stream::{Stream, StreamExt};
use uuid::Uuid;

use crate::godox;
use crate::pdu;
use crate::provision::{ProvisioningBearer, PDU_FAILED};
use crate::state::MeshState;

/// The 16-bit UUID expanded to its full Bluetooth base form.
const fn sig_uuid(short: u16) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_1000_8000_0080_5f9b_34fb_u128 | ((short as u128) << 96))
}

/// Mesh Proxy Service.
pub const MESH_PROXY_SERVICE: Uuid = sig_uuid(0x1828);
/// Mesh Proxy Data In (write commands here).
pub const MESH_PROXY_DATA_IN: Uuid = sig_uuid(0x2ADD);
/// Mesh Proxy Data Out (notifications).
pub const MESH_PROXY_DATA_OUT: Uuid = sig_uuid(0x2ADE);
/// Mesh Provisioning Service (unprovisioned nodes).
pub const MESH_PROVISIONING_SERVICE: Uuid = sig_uuid(0x1827);
/// Provisioning Data In.
pub const MESH_PROVISIONING_DATA_IN: Uuid = sig_uuid(0x2ADB);
/// Provisioning Data Out.
pub const MESH_PROVISIONING_DATA_OUT: Uuid = sig_uuid(0x2ADC);

/// A device seen while scanning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScannedDevice {
    /// Platform address / identifier as a string (MAC on Linux/Windows, UUID
    /// on macOS).
    pub address: String,
    pub name: String,
    /// Advertised service UUIDs, when the platform exposes them.
    pub services: Vec<Uuid>,
    /// Advertises the provisioning service (0x1827).
    pub unprovisioned: bool,
    /// Advertises the proxy service (0x1828).
    pub proxy: bool,
}

impl ScannedDevice {
    /// "unprovisioned" / "proxy" / "other", for the CLI's scan output.
    pub fn kind(&self) -> &'static str {
        if self.unprovisioned {
            "unprovisioned"
        } else if self.proxy {
            "proxy"
        } else {
            "other"
        }
    }
}

async fn first_adapter() -> Result<Adapter, String> {
    let manager = Manager::new().await.map_err(|e| format!("bluetooth manager: {e}"))?;
    let adapters = manager
        .adapters()
        .await
        .map_err(|e| format!("listing adapters: {e}"))?;
    adapters.into_iter().next().ok_or_else(|| "no Bluetooth adapter found".to_string())
}

/// Scan for mesh devices for `seconds`.
///
/// The filter is intentionally empty: an unprovisioned light advertising only
/// `0x1827` must show up, and some platforms hide the service list until a
/// connection is made, so filtering on services would drop real lights. We
/// classify afterwards from what was advertised.
pub async fn scan(seconds: u64) -> Result<Vec<ScannedDevice>, String> {
    let adapter = first_adapter().await?;
    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(|e| format!("start scan: {e}"))?;
    tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
    let peripherals = adapter
        .peripherals()
        .await
        .map_err(|e| format!("listing peripherals: {e}"))?;
    let _ = adapter.stop_scan().await;

    let mut seen = Vec::new();
    for p in peripherals {
        let address = p.address().to_string();
        let props = match p.properties().await {
            Ok(Some(props)) => props,
            // Rule 6: a device we cannot inspect is a skip, not a crash.
            _ => continue,
        };
        let services = props.services.clone();
        let unprovisioned = services.contains(&MESH_PROVISIONING_SERVICE);
        let proxy = services.contains(&MESH_PROXY_SERVICE);
        seen.push(ScannedDevice {
            address,
            name: props.local_name.unwrap_or_default(),
            services,
            unprovisioned,
            proxy,
        });
    }
    seen.sort_by(|a, b| a.address.cmp(&b.address));
    Ok(seen)
}

/// Find a peripheral by address string, or the first one advertising `service`.
async fn find_peripheral(address: &str, service: Uuid) -> Result<Peripheral, String> {
    let adapter = first_adapter().await?;
    if !address.is_empty() {
        // Scan briefly so the address is in the adapter's cache.
        let _ = adapter
            .start_scan(ScanFilter { services: vec![] })
            .await;
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        let _ = adapter.stop_scan().await;
        let peripherals = adapter.peripherals().await.map_err(|e| e.to_string())?;
        for p in peripherals {
            if p.address().to_string().eq_ignore_ascii_case(address) {
                return Ok(p);
            }
        }
        return Err(format!("no peripheral with address {address}"));
    }

    // No address: take the first advertising the requested service.
    adapter
        .start_scan(ScanFilter { services: vec![service] })
        .await
        .map_err(|e| format!("start scan: {e}"))?;
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    let peripherals = adapter
        .peripherals()
        .await
        .map_err(|e| e.to_string())?;
    let _ = adapter.stop_scan().await;
    for p in peripherals {
        if let Ok(true) = p.is_connected().await {
            continue;
        }
        if let Ok(Some(props)) = p.properties().await {
            if props.services.contains(&service) {
                return Ok(p);
            }
        }
    }
    Err("no device advertising the Mesh service was found".into())
}

fn characteristic(peripheral: &Peripheral, uuid: Uuid) -> Result<Characteristic, String> {
    peripheral
        .characteristics()
        .into_iter()
        .find(|c| c.uuid == uuid)
        .ok_or_else(|| format!("characteristic {uuid} not found"))
}

/// A connected Mesh Proxy bearer (0x1828).
pub struct MeshProxy {
    peripheral: Peripheral,
    data_in: Characteristic,
    data_out: Characteristic,
    notifications: std::pin::Pin<Box<dyn Stream<Item = btleplug::api::ValueNotification> + Send>>,
}

impl MeshProxy {
    /// Connect to the Mesh Proxy Service.
    ///
    /// `address == ""` means "the first device advertising 0x1828", matching
    /// the plan's CLI default.
    pub async fn connect(address: &str) -> Result<MeshProxy, String> {
        let peripheral = find_peripheral(address, MESH_PROXY_SERVICE).await?;
        if !peripheral.is_connected().await.map_err(|e| e.to_string())? {
            peripheral
                .connect()
                .await
                .map_err(|e| format!("connect: {e}"))?;
        }
        peripheral
            .discover_services()
            .await
            .map_err(|e| format!("discover services: {e}"))?;
        let data_in = characteristic(&peripheral, MESH_PROXY_DATA_IN)?;
        let data_out = characteristic(&peripheral, MESH_PROXY_DATA_OUT)?;
        peripheral
            .subscribe(&data_out)
            .await
            .map_err(|e| format!("subscribe 0x2ADE: {e}"))?;
        let notifications = peripheral
            .notifications()
            .await
            .map_err(|e| format!("notification stream: {e}"))?;
        Ok(MeshProxy {
            peripheral,
            data_in,
            data_out,
            notifications: Box::pin(notifications),
        })
    }

    /// Write a complete Proxy PDU (write without response).
    pub async fn write(&self, data: &[u8]) -> Result<(), String> {
        self.peripheral
            .write(&self.data_in, data, WriteType::WithoutResponse)
            .await
            .map_err(|e| format!("write 0x2ADD: {e}"))
    }

    /// Await the next Data Out notification.
    pub async fn next_notification(&mut self) -> Option<Vec<u8>> {
        self.notifications.next().await.map(|n| n.value)
    }

    pub async fn disconnect(&self) {
        let _ = self.peripheral.unsubscribe(&self.data_out).await;
        let _ = self.peripheral.disconnect().await;
    }

    pub fn address(&self) -> String {
        self.peripheral.address().to_string()
    }
}

/// The post-provisioning Config Server session (device-key path).
///
/// A freshly provisioned node has an empty AppKey slot, so the runtime commands
/// would be dropped. Two Config Server messages fix that:
/// `Config AppKey Add` then `Config Model App Bind` for the Telink vendor model
/// (`0x0211`/`0x0000`), both encrypted with the **device key** and sent over
/// the Proxy connection with TTL=0.
pub struct ConfigSession<'a> {
    proxy: &'a MeshProxy,
    state: &'a mut MeshState,
    /// Where to persist the sequence high-water mark.
    mesh_path: std::path::PathBuf,
}

impl<'a> ConfigSession<'a> {
    pub fn new(proxy: &'a MeshProxy, state: &'a mut MeshState, mesh_path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            proxy,
            state,
            mesh_path: mesh_path.into(),
        }
    }

    /// Send both config PDUs, persisting the sequence number before each use.
    pub async fn run(&mut self) -> Result<(), String> {
        let app_key = self.state.app_key_bytes();
        let add = godox::config_app_key_add(0, 0, &app_key);
        self.send_config_pdu(&add).await?;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let bind = godox::config_model_app_bind(
            self.state.node_address,
            0,
            godox::COMPANY_ID,
            godox::VENDOR_MODEL_ID,
        );
        self.send_config_pdu(&bind).await?;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        Ok(())
    }

    async fn send_config_pdu(&mut self, access_payload: &[u8]) -> Result<(), String> {
        let net_key = self.state.network_key_bytes();
        let device_key = self
            .state
            .device_key_bytes()
            .ok_or("mesh state has no device key; cannot use the Config Server path")?;
        let (nid, enc_key, priv_key) = crate::crypto::k2(&net_key, 0x00);

        let seq = self
            .state
            .next_seq(&self.mesh_path)
            .map_err(|e| format!("reserving sequence: {e}"))?;
        let upper = pdu::encode_device_key_pdu(
            access_payload,
            &device_key,
            self.state.iv_index,
            seq,
            self.state.provisioner_address,
            self.state.node_address,
        );
        let net = pdu::encode_network_pdu(
            nid,
            &enc_key,
            &priv_key,
            self.state.iv_index,
            false,
            0, // Config Server messages go with TTL=0
            seq,
            self.state.provisioner_address,
            self.state.node_address,
            &upper,
            4,
        );
        let proxy_pdu = pdu::encode_proxy_network(&net.bytes);
        self.proxy.write(&proxy_pdu).await
    }
}

/// A live PB-GATT provisioning bearer over the Provisioning Service.
pub struct ProvisioningGatt {
    peripheral: Peripheral,
    data_in: Characteristic,
    data_out: Characteristic,
    rx: std::pin::Pin<Box<dyn Stream<Item = btleplug::api::ValueNotification> + Send>>,
}

impl ProvisioningGatt {
    pub async fn connect(address: &str) -> Result<ProvisioningGatt, String> {
        let peripheral = find_peripheral(address, MESH_PROVISIONING_SERVICE).await?;
        if !peripheral.is_connected().await.map_err(|e| e.to_string())? {
            peripheral.connect().await.map_err(|e| format!("connect: {e}"))?;
        }
        peripheral
            .discover_services()
            .await
            .map_err(|e| format!("discover services: {e}"))?;
        let data_in = characteristic(&peripheral, MESH_PROVISIONING_DATA_IN)?;
        let data_out = characteristic(&peripheral, MESH_PROVISIONING_DATA_OUT)?;
        peripheral
            .subscribe(&data_out)
            .await
            .map_err(|e| format!("subscribe 0x2ADC: {e}"))?;
        let rx = peripheral
            .notifications()
            .await
            .map_err(|e| format!("notification stream: {e}"))?;
        Ok(ProvisioningGatt {
            peripheral,
            data_in,
            data_out,
            rx: Box::pin(rx),
        })
    }

    pub async fn disconnect(&self) {
        let _ = self.peripheral.unsubscribe(&self.data_out).await;
        let _ = self.peripheral.disconnect().await;
    }

    pub fn address(&self) -> String {
        self.peripheral.address().to_string()
    }
}

impl ProvisioningBearer for ProvisioningGatt {
    async fn send(&mut self, pdu: &[u8]) -> Result<(), String> {
        self.peripheral
            .write(&self.data_in, pdu, WriteType::WithoutResponse)
            .await
            .map_err(|e| format!("write 0x2ADB: {e}"))
    }

    async fn receive(&mut self) -> Result<(u8, Vec<u8>), String> {
        // Strip the leading proxy/SAR byte (0x03 for provisioning).
        let notification = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            self.rx.next(),
        )
        .await
        .map_err(|_| "provisioning timeout waiting for a PDU".to_string())?
        .ok_or("notification stream ended")?;
        let data = notification.value;
        if data.len() < 2 {
            return Err("provisioning notification too short".into());
        }
        // data[0] is the proxy/SAR byte; data[1] the PDU type.
        if data[1] == PDU_FAILED {
            let code = data.get(2).copied().unwrap_or(0xFF);
            return Err(format!("device reported provisioning failure {code:#04x}"));
        }
        Ok((data[1], data[2..].to_vec()))
    }
}

/// Build the proxy PDU for one Godox command. Kept here so `link.rs` stays
/// free of mesh-layer detail.
pub fn pack_command(
    access_payload: &[u8],
    state: &MeshState,
    dst: u16,
    ttl: u8,
    seq: u32,
) -> Vec<u8> {
    let net_key = state.network_key_bytes();
    let app_key = state.app_key_bytes();
    let (nid, enc_key, priv_key) = crate::crypto::k2(&net_key, 0x00);
    let upper = pdu::encode_access_pdu(access_payload, &app_key, state.iv_index, seq, state.provisioner_address, dst);
    let net = pdu::encode_network_pdu(
        nid,
        &enc_key,
        &priv_key,
        state.iv_index,
        false,
        ttl,
        seq,
        state.provisioner_address,
        dst,
        &upper,
        4,
    );
    pdu::encode_proxy_network(&net.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::MeshState;

    #[test]
    fn sig_uuids_expand_correctly() {
        assert_eq!(MESH_PROXY_SERVICE.to_string(), "00001828-0000-1000-8000-00805f9b34fb");
        assert_eq!(MESH_PROXY_DATA_IN.to_string(), "00002add-0000-1000-8000-00805f9b34fb");
        assert_eq!(MESH_PROXY_DATA_OUT.to_string(), "00002ade-0000-1000-8000-00805f9b34fb");
        assert_eq!(MESH_PROVISIONING_SERVICE.to_string(), "00001827-0000-1000-8000-00805f9b34fb");
    }

    #[test]
    fn pack_command_round_trips_through_the_layers() {
        // Build a state by hand and check the command can be decrypted back.
        let net_key = "7dd7364cd842ad18c17c2b820c84c3d6";
        let app_key = "63964771734fbd76e3b40519d1d94a48";
        let state = MeshState {
            network_key: net_key.into(),
            app_key: app_key.into(),
            provisioner_address: 1,
            node_address: 2,
            sequence_number: 0,
            iv_index: 0,
            device_key: "aa".repeat(16),
            device_address: String::new(),
            num_elements: 2,
            nodes: vec![],
            next: 0,
        };
        let access = godox::encode(&crate::LightState {
            level: 0.5,
            cct_kelvin: Some(4000.0),
            hsi: None,
        });
        let pdu = pack_command(&access, &state, 2, 10, 7);
        assert_eq!(pdu[0], pdu::PROXY_TYPE_NETWORK);

        let (nid, enc_key, priv_key) = crate::crypto::k2(&state.network_key_bytes(), 0x00);
        let _ = nid;
        let transport = pdu::decode_network_pdu(&pdu[1..], &enc_key, &priv_key, 0, 4)
            .expect("decrypt network pdu")
            .4;
        let recovered = pdu::decode_access_pdu(&transport, &state.app_key_bytes(), 0, 7, 1, 2)
            .expect("decrypt access pdu");
        assert_eq!(recovered, access);
    }
}
