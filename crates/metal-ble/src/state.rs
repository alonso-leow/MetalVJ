//! Persisted Mesh state: keys, addresses and the sequence high-water mark.
//!
//! A mesh node's replay protection list silently drops any PDU whose
//! `SEQ`/`SRC` pair it has already seen, so a controller **must never reuse a
//! sequence number**, even across a crash. Writing the file on every command
//! would make a dimmer slider a disk benchmark, so [`MeshState::next_seq_block`]
//! reserves a block of numbers up front, persists the block's upper bound
//! *before* the first number is used, then serves the block from memory. A
//! crash loses at most the unused tail of the block, which is harmless — the
//! next process resumes above it.
//!
//! The on-disk shape follows the reference `mesh_state.example.json` (flat
//! keys/addresses) so an existing `ha-godox-mesh` file imports unchanged, and
//! adds an optional `nodes` array — each `{unicast, device_key, mac, name}` —
//! so one file can describe a small rig. `nodes` is a convenience view: the
//! legacy flat fields remain authoritative when present.
//!
//! Saving is atomic: write a sibling temp file, then rename over the target.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Default number of sequence numbers claimed per disk write.
pub const DEFAULT_SEQ_BLOCK: u32 = 256;

/// One provisioned node in the mesh.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshNode {
    /// Node unicast address.
    pub unicast: u16,
    /// 16-byte Device Key as 32 hex characters (empty if never captured).
    #[serde(default)]
    pub device_key: String,
    /// Bluetooth address / platform identifier (empty when unknown).
    #[serde(default)]
    pub mac: String,
    /// Human label.
    #[serde(default)]
    pub name: String,
}

/// Persisted mesh state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeshState {
    /// 16-byte Network Key as 32 hex characters.
    pub network_key: String,
    /// 16-byte Application Key as 32 hex characters.
    pub app_key: String,
    /// Unicast source address this controller sends from.
    pub provisioner_address: u16,
    /// Unicast destination address of the primary node.
    pub node_address: u16,
    /// On disk: the **exclusive upper bound** of the reserved sequence block
    /// (the durable high-water mark, exactly as the reference `SequenceReserver`
    /// stores it). The value is written *before* the block is used, so a crash
    /// can only waste the unused tail, never reuse a sequence number.
    pub sequence_number: u32,
    /// Bluetooth Mesh IV Index.
    #[serde(default)]
    pub iv_index: u32,
    /// 16-byte Device Key as 32 hex characters (empty when unknown).
    #[serde(default)]
    pub device_key: String,
    /// Bluetooth address of the primary node.
    #[serde(default)]
    pub device_address: String,
    /// Elements the primary node occupies.
    #[serde(default = "default_num_elements")]
    pub num_elements: u16,
    /// Optional multi-node list. Empty for a single-light file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<MeshNode>,

    /// Next sequence number to *send*. On disk `sequence_number` is the
    /// reserved block's exclusive upper bound (the durable high-water mark);
    /// this transient cursor is where the next send actually picks up.
    #[serde(skip)]
    pub next: u32,
}

fn default_num_elements() -> u16 {
    2
}

/// Why a state file could not be used. Missing or malformed data is a *skip*
/// for callers (plan rule 6), so this is a return value, never a panic.
#[derive(Debug)]
pub enum StateError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Invalid(String),
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateError::Io(e) => write!(f, "mesh state io error: {e}"),
            StateError::Json(e) => write!(f, "mesh state json error: {e}"),
            StateError::Invalid(m) => write!(f, "invalid mesh state: {m}"),
        }
    }
}
impl std::error::Error for StateError {}
impl From<std::io::Error> for StateError {
    fn from(e: std::io::Error) -> Self {
        StateError::Io(e)
    }
}
impl From<serde_json::Error> for StateError {
    fn from(e: serde_json::Error) -> Self {
        StateError::Json(e)
    }
}

impl MeshState {
    /// Parse 16-byte hex helpers, validating the two required keys.
    fn validate(&self) -> Result<(), StateError> {
        for (name, value) in [("network_key", &self.network_key), ("app_key", &self.app_key)] {
            if value.len() != 32 || hex::decode(value).map(|v| v.len()) != Ok(16) {
                return Err(StateError::Invalid(format!("{name} must be 16 bytes of hex")));
            }
        }
        if !self.device_key.is_empty()
            && (self.device_key.len() != 32 || hex::decode(&self.device_key).map(|v| v.len()) != Ok(16))
        {
            return Err(StateError::Invalid("device_key must be 16 bytes of hex".into()));
        }
        Ok(())
    }

    /// Decode the Network Key into bytes (validated on load/construct).
    pub fn network_key_bytes(&self) -> [u8; 16] {
        self.key_bytes(&self.network_key)
    }

    /// Decode the Application Key into bytes.
    pub fn app_key_bytes(&self) -> [u8; 16] {
        self.key_bytes(&self.app_key)
    }

    /// Decode the Device Key into bytes, or `None` when unset.
    pub fn device_key_bytes(&self) -> Option<[u8; 16]> {
        if self.device_key.is_empty() {
            None
        } else {
            Some(self.key_bytes(&self.device_key))
        }
    }

    fn key_bytes(&self, hexstr: &str) -> [u8; 16] {
        let v = hex::decode(hexstr).unwrap_or_default();
        let mut out = [0u8; 16];
        let n = v.len().min(16);
        out[..n].copy_from_slice(&v[..n]);
        out
    }

    /// Load from a JSON file. A missing file is an error the caller may treat
    /// as a skip.
    pub fn load(path: impl AsRef<Path>) -> Result<MeshState, StateError> {
        let text = metal_vfs::read_to_string(path)?;
        let mut state: MeshState = serde_json::from_str(&text)?;
        state.validate()?;
        // Resume the in-memory cursor at the persisted high-water mark.
        state.next = state.sequence_number;
        Ok(state)
    }

    /// Save atomically: write a sibling temp file then rename over the target.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), StateError> {
        self.validate()?;
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                metal_vfs::create_dir_all(parent)?;
            }
        }
        let tmp = temp_sibling(path);
        let text = serde_json::to_string_pretty(self)?;
        metal_vfs::write(&tmp, text.as_bytes())?;
        // Rename is atomic on the same filesystem; fall back to a plain copy if
        // the platform refuses (e.g. some networked mounts).
        if metal_vfs::rename(&tmp, path).is_err() {
            metal_vfs::write(path, text.as_bytes())?;
            let _ = metal_vfs::remove_file(&tmp);
        }
        Ok(())
    }

    /// Reserve (and durably persist) a new sequence block.
    ///
    /// Persists an aligned exclusive upper bound **before** the block is used,
    /// exactly as the reference `SequenceReserver` does. On a write failure the
    /// in-memory cursor is left untouched so the caller can abort rather than
    /// reuse numbers it did not claim.
    pub fn reserve_seq_block(
        &mut self,
        path: impl AsRef<Path>,
        block: u32,
    ) -> Result<(), StateError> {
        let block = block.max(1);
        let floor = self.next;
        let new_hwm = (floor / block + 1) * block;
        let old_hwm = self.sequence_number;
        self.sequence_number = new_hwm;
        if let Err(e) = self.save(path) {
            self.sequence_number = old_hwm;
            return Err(e);
        }
        Ok(())
    }

    /// Return the next sequence number, reserving a block if the current one is
    /// exhausted. The block boundary is written to disk before use.
    pub fn next_seq_block(
        &mut self,
        path: impl AsRef<Path>,
        block: u32,
    ) -> Result<u32, StateError> {
        if self.next >= self.sequence_number {
            self.reserve_seq_block(path, block)?;
        }
        let value = self.next;
        self.next += 1;
        Ok(value)
    }

    /// Convenience: reserve with [`DEFAULT_SEQ_BLOCK`].
    pub fn next_seq(&mut self, path: impl AsRef<Path>) -> Result<u32, StateError> {
        self.next_seq_block(path, DEFAULT_SEQ_BLOCK)
    }

    /// Primary node's unicast address.
    pub fn primary_unicast(&self) -> u16 {
        self.node_address
    }

    /// Resolve a destination: an explicit override, else the first entry in
    /// `nodes`, else the flat `node_address`.
    pub fn destination(&self, node: Option<u16>) -> u16 {
        node.or_else(|| self.nodes.first().map(|n| n.unicast))
            .unwrap_or(self.node_address)
    }
}

fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".tmp");
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.join(name),
        _ => PathBuf::from(name),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample() -> MeshState {
        MeshState {
            network_key: "00".repeat(16),
            app_key: "11".repeat(16),
            provisioner_address: 1,
            node_address: 2,
            sequence_number: 10,
            iv_index: 0,
            device_key: "aa".repeat(16),
            device_address: "AA:BB".into(),
            num_elements: 2,
            nodes: vec![],
            next: 10,
        }
    }

    #[test]
    fn save_load_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mesh_state.json");
        let state = sample();
        state.save(&path).unwrap();
        let back = MeshState::load(&path).unwrap();
        assert_eq!(back.network_key, state.network_key);
        assert_eq!(back.sequence_number, 10);
        assert_eq!(back.next, 10);
    }

    #[test]
    fn imports_the_example_json_shape() {
        // Exactly the fields of ha-godox-mesh's mesh_state.example.json.
        let json = r#"{
            "app_key": "11111111111111111111111111111111",
            "device_key": "",
            "iv_index": 0,
            "network_key": "00000000000000000000000000000000",
            "node_address": 2,
            "provisioner_address": 1,
            "sequence_number": 300000
        }"#;
        let s: MeshState = serde_json::from_str(json).unwrap();
        assert!(s.validate().is_ok());
        assert_eq!(s.sequence_number, 300000);
        assert_eq!(s.node_address, 2);
        assert!(s.nodes.is_empty());
        assert_eq!(s.destination(Some(7)), 7);
        assert_eq!(s.destination(None), 2);
    }

    #[test]
    fn extra_unknown_fields_are_ignored_not_fatal() {
        // The reference file carries a device_address; unknown keys must not
        // crash a load (plan rule 6: malformed is a skip, not a panic).
        let json = r#"{
            "network_key": "00000000000000000000000000000000",
            "app_key": "11111111111111111111111111111111",
            "provisioner_address": 1, "node_address": 2,
            "sequence_number": 1, "iv_index": 0,
            "some_future_field": {"nested": true}
        }"#;
        let s: MeshState = serde_json::from_str(json).unwrap();
        assert!(s.validate().is_ok());
    }

    #[test]
    fn missing_file_is_an_error_not_a_panic() {
        let dir = tempdir().unwrap();
        let err = MeshState::load(dir.path().join("nope.json")).unwrap_err();
        assert!(matches!(err, StateError::Io(_)));
    }

    #[test]
    fn malformed_key_is_rejected() {
        let mut s = sample();
        s.app_key = "zz".into();
        assert!(matches!(s.validate(), Err(StateError::Invalid(_))));
    }

    #[test]
    fn reserve_persists_before_use_and_serves_the_block() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mesh_state.json");
        let mut s = sample();
        s.sequence_number = 100;
        s.next = 100;

        // First take: reserves [100, 150).
        let first = s.next_seq_block(&path, 50).unwrap();
        assert_eq!(first, 100);
        // The persisted file already knows the block's end, before 100 was used.
        let on_disk = MeshState::load(&path).unwrap();
        assert_eq!(on_disk.sequence_number, 150, "high-water mark persisted before use");
        assert_eq!(s.sequence_number, 150);

        // The rest of the block is served from memory without a write.
        for expect in 101..150 {
            assert_eq!(s.next_seq_block(&path, 50).unwrap(), expect);
        }
        // Crossing the boundary reserves the next block.
        assert_eq!(s.next_seq_block(&path, 50).unwrap(), 150);
        assert_eq!(s.sequence_number, 200);
    }

    #[test]
    fn a_crash_reuses_no_sequence_number() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mesh_state.json");
        let mut s = sample();
        s.sequence_number = 1000;
        s.next = 1000;

        // Process 1 takes 20 of a 64-block, then "crashes" (state dropped).
        let mut used = vec![];
        for _ in 0..20 {
            used.push(s.next_seq_block(&path, 64).unwrap());
        }
        drop(s);

        // Process 2 loads the file; its first number must exceed every used one.
        let mut s2 = MeshState::load(&path).unwrap();
        let resumed = s2.next_seq_block(&path, 64).unwrap();
        assert!(
            resumed > *used.iter().max().unwrap(),
            "resumed {resumed} must be above {used:?}"
        );
    }

    #[test]
    fn atomic_save_leaves_no_temp_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mesh_state.json");
        sample().save(&path).unwrap();
        let strays: Vec<_> = metal_vfs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(strays.is_empty(), "left a temp file behind: {strays:?}");
    }

    #[test]
    fn nodes_list_drives_destination() {
        let mut s = sample();
        s.nodes = vec![
            MeshNode { unicast: 0x0b0c, device_key: String::new(), mac: "x".into(), name: "key".into() },
        ];
        // An explicit node still wins.
        assert_eq!(s.destination(Some(1)), 1);
        assert_eq!(s.destination(None), 0x0b0c);
    }
}
