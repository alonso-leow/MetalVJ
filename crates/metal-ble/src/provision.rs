//! PB-GATT provisioning: the six-step exchange that puts a light on the mesh.
//!
//! The provisioning protocol (Mesh Profile §5.4) runs over the Provisioning
//! bearer (service `0x1827`, Data In `0x2ADB`, Data Out `0x2ADC`) and is a
//! fixed dialogue:
//!
//! ```text
//!   Invite            →          Capabilities      ←
//!   Start, PublicKey  →          PublicKey         ←
//!   Confirmation      →          Confirmation      ←
//!   Random            →          Random            ←
//!   Data (net key, addresses)  → Complete          ←
//! ```
//!
//! All key material comes out of ECDH over P-256 (the algorithm the lights
//! negotiate: `BTM_ECDH_P256_CMAC_AES128_AES_CCM`, No-OOB):
//!
//! * `ConfirmationSalt = s1(ConfirmationInputs)`
//! * `ConfirmationKey  = k1(ECDHSecret, ConfirmationSalt, "prck")`
//! * `ProvisioningSalt = s1(ConfirmationSalt || RandomProvisioner || RandomDevice)`
//! * `SessionKey        = k1(ECDHSecret, ProvisioningSalt, "prsk")`
//! * `SessionNonce      = k1(ECDHSecret, ProvisioningSalt, "prsn")[3..]`
//! * `DeviceKey         = k1(ECDHSecret, ProvisioningSalt, "prdk")`
//!
//! The derivation helpers are pure and are pinned to Mesh Profile §8.17 sample
//! data in the tests. The orchestration is generic over a [`ProvisioningBearer`]
//! so it can be driven by a fake in tests and by [`crate::transport`] on radio.
//!
//! After provisioning, the node's AppKey slot is empty; `crate::transport`'s
//! `ConfigSession` binds the application key with Config Server messages over
//! the Mesh Proxy connection.

use crate::crypto::{aes_cmac, ccm_encrypt, k1, s1};
use crate::pdu::{encode_proxy_provisioning, ProxyAssembly, ProxyMessage};

// Provisioning PDU types (Mesh Profile §5.4.1).
pub const PDU_INVITE: u8 = 0x00;
pub const PDU_CAPABILITIES: u8 = 0x01;
pub const PDU_START: u8 = 0x02;
pub const PDU_PUBLIC_KEY: u8 = 0x03;
pub const PDU_INPUT_COMPLETE: u8 = 0x04;
pub const PDU_CONFIRMATION: u8 = 0x05;
pub const PDU_RANDOM: u8 = 0x06;
pub const PDU_DATA: u8 = 0x07;
pub const PDU_COMPLETE: u8 = 0x08;
pub const PDU_FAILED: u8 = 0x09;

/// The device reported provisioning failure with this error code.
#[derive(Debug)]
pub struct ProvisioningFailed(pub u8);

impl std::fmt::Display for ProvisioningFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "provisioning failed, device error code {:#04x}", self.0)
    }
}
impl std::error::Error for ProvisioningFailed {}

/// Anything that can carry provisioning PDUs. Implemented for real BLE by
/// `crate::transport`; tests use an in-memory queue.
#[allow(async_fn_in_trait)]
pub trait ProvisioningBearer {
    /// Send a complete provisioning PDU (already proxy-framed).
    async fn send(&mut self, pdu: &[u8]) -> Result<(), String>;
    /// Wait for the next provisioning PDU `(pdu_type, payload)`, with a timeout.
    async fn receive(&mut self) -> Result<(u8, Vec<u8>), String>;
}

/// Parsed Provisioning Capabilities PDU (device → provisioner).
#[derive(Clone, Debug, PartialEq)]
pub struct Capabilities {
    pub num_elements: u8,
    pub algorithms: u16,
    pub pub_key_type: u8,
    pub static_oob_type: u8,
    pub output_oob_size: u8,
    pub output_oob_action: u16,
    pub input_oob_size: u8,
    pub input_oob_action: u16,
    /// The raw 11-byte payload, needed verbatim for ConfirmationInputs.
    pub raw: Vec<u8>,
}

/// Build the Invite PDU (proxy-framed).
pub fn build_invite(attention_duration: u8) -> Vec<u8> {
    encode_proxy_provisioning(PDU_INVITE, &[attention_duration])
}

/// Build the Start PDU with No-OOB defaults.
pub fn build_start() -> Vec<u8> {
    encode_proxy_provisioning(PDU_START, &[0x00, 0x00, 0x00, 0x00, 0x00])
}

/// Build the Public Key PDU from a raw 64-byte X||Y key.
pub fn build_public_key(pub_key: &[u8; 64]) -> Vec<u8> {
    encode_proxy_provisioning(PDU_PUBLIC_KEY, pub_key)
}

/// Build the Confirmation PDU.
pub fn build_confirmation(confirmation: &[u8; 16]) -> Vec<u8> {
    encode_proxy_provisioning(PDU_CONFIRMATION, confirmation)
}

/// Build the Random PDU.
pub fn build_random(random: &[u8; 16]) -> Vec<u8> {
    encode_proxy_provisioning(PDU_RANDOM, random)
}

/// Build the Data PDU from the 33-byte AES-CCM ciphertext.
pub fn build_data(encrypted: &[u8]) -> Vec<u8> {
    assert_eq!(encrypted.len(), 33, "provisioning Data is 25 bytes + 8-byte MIC");
    encode_proxy_provisioning(PDU_DATA, encrypted)
}

/// Parse a Capabilities payload (PDU type byte already stripped).
pub fn parse_capabilities(payload: &[u8]) -> Result<Capabilities, String> {
    if payload.len() != 11 {
        return Err(format!("Capabilities must be 11 bytes, got {}", payload.len()));
    }
    Ok(Capabilities {
        num_elements: payload[0],
        algorithms: u16::from_be_bytes([payload[1], payload[2]]),
        pub_key_type: payload[3],
        static_oob_type: payload[4],
        output_oob_size: payload[5],
        output_oob_action: u16::from_be_bytes([payload[6], payload[7]]),
        input_oob_size: payload[8],
        input_oob_action: u16::from_be_bytes([payload[9], payload[10]]),
        raw: payload.to_vec(),
    })
}

/// `ConfirmationInputs = Invite || Capabilities || Start || PublicKeyProv || PublicKeyDev`
/// (145 bytes).
pub fn confirmation_inputs(
    invite_payload: &[u8],
    capabilities_payload: &[u8],
    start_payload: &[u8],
    provisioner_pubkey: &[u8; 64],
    device_pubkey: &[u8; 64],
) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(145);
    out.extend_from_slice(invite_payload);
    out.extend_from_slice(capabilities_payload);
    out.extend_from_slice(start_payload);
    out.extend_from_slice(provisioner_pubkey);
    out.extend_from_slice(device_pubkey);
    if out.len() != 145 {
        return Err(format!("ConfirmationInputs must be 145 bytes, got {}", out.len()));
    }
    Ok(out)
}

/// `ConfirmationSalt = s1(ConfirmationInputs)`.
pub fn confirmation_salt(inputs: &[u8]) -> [u8; 16] {
    s1(inputs)
}

/// `ConfirmationKey = k1(ECDHSecret, ConfirmationSalt, "prck")`.
pub fn confirmation_key(ecdh_secret: &[u8], confirmation_salt: &[u8; 16]) -> [u8; 16] {
    k1(ecdh_secret, confirmation_salt, b"prck")
}

/// `Confirmation = AES-CMAC(ConfirmationKey, Random || AuthValue)`.
pub fn confirmation_value(
    ecdh_secret: &[u8],
    confirmation_salt: &[u8; 16],
    random16: &[u8; 16],
    auth_value: &[u8; 16],
) -> [u8; 16] {
    let ck = confirmation_key(ecdh_secret, confirmation_salt);
    let mut input = Vec::with_capacity(32);
    input.extend_from_slice(random16);
    input.extend_from_slice(auth_value);
    aes_cmac(&ck, &input)
}

/// `ProvisioningSalt = s1(ConfirmationSalt || RandomProvisioner || RandomDevice)`.
pub fn provision_salt(
    confirmation_salt: &[u8; 16],
    provisioner_random: &[u8; 16],
    device_random: &[u8; 16],
) -> [u8; 16] {
    let mut m = Vec::with_capacity(48);
    m.extend_from_slice(confirmation_salt);
    m.extend_from_slice(provisioner_random);
    m.extend_from_slice(device_random);
    s1(&m)
}

/// `SessionKey = k1(ECDHSecret, ProvisioningSalt, "prsk")`.
pub fn session_key(ecdh_secret: &[u8], provision_salt: &[u8; 16]) -> [u8; 16] {
    k1(ecdh_secret, provision_salt, b"prsk")
}

/// `SessionNonce = k1(ECDHSecret, ProvisioningSalt, "prsn")[3..]` (13 bytes).
pub fn session_nonce(ecdh_secret: &[u8], provision_salt: &[u8; 16]) -> [u8; 13] {
    let full = k1(ecdh_secret, provision_salt, b"prsn");
    let mut n = [0u8; 13];
    n.copy_from_slice(&full[3..]);
    n
}

/// `DeviceKey = k1(ECDHSecret, ProvisioningSalt, "prdk")`.
pub fn device_key(ecdh_secret: &[u8], provision_salt: &[u8; 16]) -> [u8; 16] {
    k1(ecdh_secret, provision_salt, b"prdk")
}

/// Encrypt the 25-byte Provisioning Data plaintext into 33 bytes.
///
/// Plaintext: `NetworkKey(16) || NetKeyIndex(2) || Flags(1) || IVIndex(4) || UnicastAddress(2)`.
#[allow(clippy::too_many_arguments)]
pub fn encrypt_provisioning_data(
    net_key: &[u8; 16],
    key_index: u16,
    flags: u8,
    iv_index: u32,
    unicast_address: u16,
    session_key: &[u8; 16],
    session_nonce: &[u8; 13],
) -> Vec<u8> {
    let mut plain = Vec::with_capacity(25);
    plain.extend_from_slice(net_key);
    plain.extend_from_slice(&key_index.to_be_bytes());
    plain.push(flags);
    plain.extend_from_slice(&iv_index.to_be_bytes());
    plain.extend_from_slice(&unicast_address.to_be_bytes());
    ccm_encrypt(session_key, session_nonce, &plain, 8)
}

/// A freshly generated P-256 keypair for one provisioning session.
pub struct Keypair {
    pub secret: p256::SecretKey,
    /// Raw uncompressed X||Y, 64 bytes (no `0x04` tag).
    pub public: [u8; 64],
}

/// Generate a random P-256 keypair using the OS CSPRNG.
///
/// The 32-byte scalar comes straight from `getrandom`; `SecretKey::from_slice`
/// rejects the (astronomically unlikely) out-of-range values and we retry.
pub fn generate_p256_keypair() -> Result<Keypair, String> {
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    for _ in 0..8 {
        let mut scalar = [0u8; 32];
        getrandom::getrandom(&mut scalar).map_err(|e| format!("rng failure: {e}"))?;
        let secret = match p256::SecretKey::from_slice(&scalar) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let point = secret.public_key().to_encoded_point(false);
        let mut public = [0u8; 64];
        public.copy_from_slice(&point.as_bytes()[1..]);
        return Ok(Keypair { secret, public });
    }
    Err("could not generate a valid P-256 scalar".into())
}

impl Keypair {
    /// Build a keypair from a 32-byte private scalar. Used by tests to pin the
    /// §8.17 sample vectors; production paths use [`generate_p256_keypair`].
    pub fn from_scalar(scalar: &[u8]) -> Result<Keypair, String> {
        use p256::elliptic_curve::sec1::ToEncodedPoint;
        let secret = p256::SecretKey::from_slice(scalar)
            .map_err(|e| format!("not a valid P-256 scalar: {e}"))?;
        let point = secret.public_key().to_encoded_point(false);
        let mut public = [0u8; 64];
        public.copy_from_slice(&point.as_bytes()[1..]);
        Ok(Keypair { secret, public })
    }
}

/// Compute the 32-byte ECDH shared secret against a raw 64-byte public key.
pub fn ecdh_shared_secret(secret: &p256::SecretKey, peer_public: &[u8; 64]) -> Result<[u8; 32], String> {
    // Re-add the uncompressed-point tag the wire omits.
    let mut sec1 = Vec::with_capacity(65);
    sec1.push(0x04);
    sec1.extend_from_slice(peer_public);
    let peer = p256::PublicKey::from_sec1_bytes(&sec1)
        .map_err(|e| format!("device public key is not a valid P-256 point: {e}"))?;
    let shared = p256::ecdh::diffie_hellman(secret.to_nonzero_scalar(), peer.as_affine());
    let mut out = [0u8; 32];
    out.copy_from_slice(shared.raw_secret_bytes());
    Ok(out)
}

/// Outcome of a successful provisioning exchange.
#[derive(Clone, Debug)]
pub struct Provisioned {
    pub net_key: [u8; 16],
    pub device_key: [u8; 16],
    pub unicast_address: u16,
    pub num_elements: u16,
    pub iv_index: u32,
}

/// The pure state machine of a provisioning session.
///
/// It owns only the local keypair and the net key; all I/O goes through the
/// bearer. Steps that need randomness take it from the OS by default; the
/// [`ProvisioningSession::exchange_with`] helper lets tests inject both randoms
/// so the §8.17 sample data can drive the whole flow.
pub struct ProvisioningSession {
    pub net_key: [u8; 16],
    pub key_index: u16,
    pub iv_index: u32,
    pub unicast_address: u16,
    pub provisioner_address: u16,
    pub attention_duration: u8,
    pub num_elements: u16,
}

impl ProvisioningSession {
    pub fn new(
        net_key: [u8; 16],
        key_index: u16,
        iv_index: u32,
        unicast_address: u16,
    ) -> Self {
        Self {
            net_key,
            key_index,
            iv_index,
            unicast_address,
            provisioner_address: 0x0001,
            attention_duration: 0,
            num_elements: 2,
        }
    }

    /// Run the exchange against a bearer, generating random values from the OS.
    pub async fn run(&mut self, bearer: &mut impl ProvisioningBearer) -> Result<Provisioned, String> {
        let mut prov_random = [0u8; 16];
        getrandom::getrandom(&mut prov_random).map_err(|e| format!("rng failure: {e}"))?;
        self.exchange_with(bearer, prov_random).await
    }

    /// Run the exchange with an explicit provisioner random (for tests / replay).
    pub async fn exchange_with(
        &mut self,
        bearer: &mut impl ProvisioningBearer,
        prov_random: [u8; 16],
    ) -> Result<Provisioned, String> {
        let keypair = generate_p256_keypair()?;
        self.exchange_parts(bearer, &keypair, prov_random).await
    }

    /// The exchange body, with the keypair supplied. Public so a test can pin
    /// the provisioner key to a spec sample vector; production callers use
    /// [`ProvisioningSession::run`].
    pub async fn exchange_parts(
        &mut self,
        bearer: &mut impl ProvisioningBearer,
        keypair: &Keypair,
        prov_random: [u8; 16],
    ) -> Result<Provisioned, String> {
        // 1. Invite → Capabilities
        let invite_payload = [self.attention_duration];
        bearer.send(&build_invite(self.attention_duration)).await?;
        let (t, payload) = bearer.receive().await?;
        if t != PDU_CAPABILITIES {
            return Err(format!("expected Capabilities, got PDU type {t:#04x}"));
        }
        let caps = parse_capabilities(&payload)?;
        self.num_elements = caps.num_elements.max(1) as u16;

        // 2. Start + PublicKey → device PublicKey
        let start_payload = [0x00u8, 0x00, 0x00, 0x00, 0x00];
        bearer.send(&build_start()).await?;
        bearer.send(&build_public_key(&keypair.public)).await?;
        let (t, payload) = bearer.receive().await?;
        if t != PDU_PUBLIC_KEY {
            return Err(format!("expected PublicKey, got PDU type {t:#04x}"));
        }
        if payload.len() != 64 {
            return Err(format!("device PublicKey must be 64 bytes, got {}", payload.len()));
        }
        let mut device_pub = [0u8; 64];
        device_pub.copy_from_slice(&payload);

        // 3. ECDH + Confirmation
        let secret = ecdh_shared_secret(&keypair.secret, &device_pub)?;
        let inputs = confirmation_inputs(
            &invite_payload,
            &caps.raw,
            &start_payload,
            &keypair.public,
            &device_pub,
        )?;
        let conf_salt = confirmation_salt(&inputs);
        let our_conf = confirmation_value(&secret, &conf_salt, &prov_random, &[0u8; 16]);
        bearer.send(&build_confirmation(&our_conf)).await?;

        let (t, payload) = bearer.receive().await?;
        if t != PDU_CONFIRMATION {
            return Err(format!("expected Confirmation, got PDU type {t:#04x}"));
        }
        if payload.len() != 16 {
            return Err(format!("device Confirmation must be 16 bytes, got {}", payload.len()));
        }
        let mut device_conf = [0u8; 16];
        device_conf.copy_from_slice(&payload);

        // 4. Random
        bearer.send(&build_random(&prov_random)).await?;
        let (t, payload) = bearer.receive().await?;
        if t != PDU_RANDOM {
            return Err(format!("expected Random, got PDU type {t:#04x}"));
        }
        if payload.len() != 16 {
            return Err(format!("device Random must be 16 bytes, got {}", payload.len()));
        }
        let mut device_random = [0u8; 16];
        device_random.copy_from_slice(&payload);

        // Verify the device's confirmation; a mismatch means a wrong secret.
        let expected = confirmation_value(&secret, &conf_salt, &device_random, &[0u8; 16]);
        if expected != device_conf {
            return Err("device confirmation did not verify".into());
        }

        // 5. Derive session material + send Data
        let psalt = provision_salt(&conf_salt, &prov_random, &device_random);
        let skey = session_key(&secret, &psalt);
        let snonce = session_nonce(&secret, &psalt);
        let dkey = device_key(&secret, &psalt);
        let encrypted = encrypt_provisioning_data(
            &self.net_key,
            self.key_index,
            0,
            self.iv_index,
            self.unicast_address,
            &skey,
            &snonce,
        );
        bearer.send(&build_data(&encrypted)).await?;

        // 6. Complete
        let (t, _) = bearer.receive().await?;
        if t != PDU_COMPLETE {
            return Err(format!("expected Complete, got PDU type {t:#04x}"));
        }

        Ok(Provisioned {
            net_key: self.net_key,
            device_key: dkey,
            unicast_address: self.unicast_address,
            num_elements: self.num_elements,
            iv_index: self.iv_index,
        })
    }
}

/// Interpret one Proxy Data Out notification as a provisioning PDU.
pub fn provisioning_from_notification(asm: &mut ProxyAssembly, data: &[u8]) -> Option<(u8, Vec<u8>)> {
    match asm.push(data)? {
        ProxyMessage::Provisioning(t, payload) => Some((t, payload)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::s1 as salt;
    use std::collections::VecDeque;

    fn h(s: &str) -> Vec<u8> {
        hex::decode(s).unwrap()
    }

    // ---- §8.17.1 provisioning sample data ---------------------------------
    #[test]
    fn spec_8_17_derivations() {
        let secret = h("ab85843a2f6d883f62e5684b38e307335fe6e1945ecd19604105c6f23221eb69");
        let invite = h("00");
        let caps = h("0100010000000000000000");
        let start = h("0000000000");
        let prov_pub = {
            let mut v = h("2c31a47b5779809ef44cb5eaaf5c3e43d5f8faad4a8794cb987e9b03745c78dd");
            v.extend(h("919512183898dfbecd52e2408e43871fd021109117bd3ed4eaf8437743715d4f"));
            let mut a = [0u8; 64];
            a.copy_from_slice(&v);
            a
        };
        let dev_pub = {
            let mut v = h("f465e43ff23d3f1b9dc7dfc04da8758184dbc966204796eccf0d6cf5e16500cc");
            v.extend(h("0201d048bcbbd899eeefc424164e33c201c2b010ca6b4d43a8a155cad8ecb279"));
            let mut a = [0u8; 64];
            a.copy_from_slice(&v);
            a
        };

        let inputs = confirmation_inputs(&invite, &caps, &start, &prov_pub, &dev_pub).unwrap();
        assert_eq!(
            hex::encode(&inputs),
            "00010001000000000000000000000000002c31a47b5779809ef44cb5eaaf5c3e43d5f8faad4a8794cb987e9b03745c78dd919512183898dfbecd52e2408e43871fd021109117bd3ed4eaf8437743715d4ff465e43ff23d3f1b9dc7dfc04da8758184dbc966204796eccf0d6cf5e16500cc0201d048bcbbd899eeefc424164e33c201c2b010ca6b4d43a8a155cad8ecb279"
        );
        let csalt = confirmation_salt(&inputs);
        assert_eq!(csalt.to_vec(), h("5faabe187337c71cc6c973369dcaa79a"));
        let ckey = confirmation_key(&secret, &csalt);
        assert_eq!(ckey.to_vec(), h("e31fe046c68ec339c425fc6629f0336f"));

        let rp: [u8; 16] = h("8b19ac31d58b124c946209b5db1021b9").try_into().unwrap();
        let rd: [u8; 16] = h("55a2a2bca04cd32ff6f346bd0a0c1a3a").try_into().unwrap();
        assert_eq!(
            confirmation_value(&secret, &csalt, &rp, &[0u8; 16]).to_vec(),
            h("b38a114dfdca1fe153bd2c1e0dc46ac2")
        );
        assert_eq!(
            confirmation_value(&secret, &csalt, &rd, &[0u8; 16]).to_vec(),
            h("eeba521c196b52cc2e37aa40329f554e")
        );

        let psalt = provision_salt(&csalt, &rp, &rd);
        assert_eq!(psalt.to_vec(), h("a21c7d45f201cf9489a2fb57145015b4"));
        assert_eq!(session_key(&secret, &psalt).to_vec(), h("c80253af86b33dfa450bbdb2a191fea3"));
        assert_eq!(session_nonce(&secret, &psalt).to_vec(), h("da7ddbe78b5f62b81d6847487e"));
        assert_eq!(device_key(&secret, &psalt).to_vec(), h("0520adad5e0142aa3e325087b4ec16d8"));

        let net_key: [u8; 16] = h("efb2255e6422d330088e09bb015ed707").try_into().unwrap();
        let data = encrypt_provisioning_data(
            &net_key,
            0x0567,
            0,
            0x0102_0304,
            0x0b0c,
            &session_key(&secret, &psalt),
            &session_nonce(&secret, &psalt),
        );
        assert_eq!(
            hex::encode(&data),
            "d0bd7f4a89a2ff6222af59a90a60ad58acfe3123356f5cec2973e0ec50783b10c7"
        );
    }

    #[test]
    fn confirmation_salt_is_s1_of_inputs() {
        let inputs = vec![7u8; 145];
        assert_eq!(confirmation_salt(&inputs), salt(&inputs));
    }

    #[test]
    fn pdu_builders_have_the_right_framing() {
        assert_eq!(build_invite(0), vec![0x03, 0x00, 0x00]);
        assert_eq!(build_invite(5).len(), 3);
        assert_eq!(build_start(), vec![0x03, 0x02, 0, 0, 0, 0, 0]);
        assert_eq!(build_public_key(&[0u8; 64]).len(), 66);
        assert_eq!(build_confirmation(&[0u8; 16]).len(), 18);
        assert_eq!(build_random(&[0u8; 16]).len(), 18);
        assert_eq!(build_data(&[0u8; 33]).len(), 35);
    }

    #[test]
    fn ecdh_is_symmetric_and_32_bytes() {
        let a = generate_p256_keypair().unwrap();
        let b = generate_p256_keypair().unwrap();
        let ab = ecdh_shared_secret(&a.secret, &b.public).unwrap();
        let ba = ecdh_shared_secret(&b.secret, &a.public).unwrap();
        assert_eq!(ab.len(), 32);
        assert_eq!(ab, ba, "ECDH must agree in both directions");
    }

    #[test]
    fn confirmation_inputs_length_is_enforced() {
        // 1 + 10 (wrong) + 5 + 64 + 64 = 144 → error, not a panic.
        let err = confirmation_inputs(&[0], &[0u8; 10], &[0u8; 5], &[0u8; 64], &[0u8; 64]).unwrap_err();
        assert!(err.contains("145"));
    }

    // ---- end-to-end against a fake device --------------------------------

    /// A device that actually performs its half of the §8.17 exchange: it owns
    /// the sample device private key and the two randoms, so it can compute the
    /// ECDH secret once it sees the provisioner's public key and answer every
    /// step truthfully. That makes this a genuine round trip, not a script.
    struct FakeDeviceBearer {
        rx: VecDeque<(u8, Vec<u8>)>,
        /// Device private scalar (the §8.17 `PrivateKeyDevice`).
        device_secret: p256::SecretKey,
        /// Device public X||Y (the §8.17 `PublicKeyDevice`).
        device_public: [u8; 64],
        random_device: [u8; 16],
        /// Filled when Start/PublicKey arrive.
        saw_public_key: Option<[u8; 64]>,
    }

    impl ProvisioningBearer for FakeDeviceBearer {
        async fn send(&mut self, pdu: &[u8]) -> Result<(), String> {
            if pdu.first() != Some(&0x03) {
                return Err("not a provisioning frame".into());
            }
            let (t, payload) = (pdu[1], pdu[2..].to_vec());
            match t {
                PDU_INVITE => {
                    self.rx.push_back((
                        PDU_CAPABILITIES,
                        vec![0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
                    ));
                }
                PDU_START => {}
                PDU_PUBLIC_KEY => {
                    if payload.len() != 64 {
                        return Err("PublicKey must be 64 bytes".into());
                    }
                    let mut pubkey = [0u8; 64];
                    pubkey.copy_from_slice(&payload);
                    self.saw_public_key = Some(pubkey);
                    self.rx.push_back((PDU_PUBLIC_KEY, self.device_public.to_vec()));
                }
                PDU_CONFIRMATION => {
                    // Compute the device's Confirmation from the shared secret.
                    let prov_pub = self.saw_public_key.unwrap();
                    let secret = ecdh_shared_secret(&self.device_secret, &prov_pub).unwrap();
                    // Reconstruct ConfirmationInputs exactly as the device does.
                    let invite = vec![0x00u8];
                    let caps =
                        vec![0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
                    let start = vec![0x00u8; 5];
                    let inputs =
                        confirmation_inputs(&invite, &caps, &start, &prov_pub, &self.device_public)
                            .unwrap();
                    let csalt = confirmation_salt(&inputs);
                    let dev_conf = confirmation_value(&secret, &csalt, &self.random_device, &[0u8; 16]);
                    self.rx.push_back((PDU_CONFIRMATION, dev_conf.to_vec()));
                }
                PDU_RANDOM => {
                    self.rx.push_back((PDU_RANDOM, self.random_device.to_vec()));
                }
                PDU_DATA => {
                    self.rx.push_back((PDU_COMPLETE, vec![]));
                }
                other => return Err(format!("unexpected PDU type {other:#04x}")),
            }
            Ok(())
        }
        async fn receive(&mut self) -> Result<(u8, Vec<u8>), String> {
            self.rx.pop_front().ok_or_else(|| "no more PDUs".to_string())
        }
    }

    #[tokio::test]
    async fn full_exchange_against_a_real_fake_device() {
        use p256::elliptic_curve::sec1::ToEncodedPoint;
        let device_secret = p256::SecretKey::from_slice(&h(
            "529aa0670d72cd6497502ed473502b037e8803b5c60829a5a3caa219505530ba",
        ))
        .unwrap();
        let mut device_public = [0u8; 64];
        device_public.copy_from_slice(&device_secret.public_key().to_encoded_point(false).as_bytes()[1..]);
        let random_device: [u8; 16] = h("55a2a2bca04cd32ff6f346bd0a0c1a3a").try_into().unwrap();

        let mut bearer = FakeDeviceBearer {
            rx: VecDeque::new(),
            device_secret,
            device_public,
            random_device,
            saw_public_key: None,
        };

        let net_key: [u8; 16] = h("efb2255e6422d330088e09bb015ed707").try_into().unwrap();
        let mut session = ProvisioningSession::new(net_key, 0x0567, 0x0102_0304, 0x0b0c);
        let prov_random: [u8; 16] = h("8b19ac31d58b124c946209b5db1021b9").try_into().unwrap();
        // Pin the provisioner key to the §8.17 sample so the derived device key
        // is the one the spec states, end to end.
        let keypair = Keypair::from_scalar(
            &h("06a516693c9aa31a6084545d0c5db641b48572b97203ddffb7ac73f7d0457663"),
        )
        .unwrap();
        let out = session
            .exchange_parts(&mut bearer, &keypair, prov_random)
            .await
            .unwrap();

        assert_eq!(out.unicast_address, 0x0b0c);
        assert_eq!(out.net_key, net_key);
        // The device key must be the §8.17 value for this secret+randoms.
        assert_eq!(out.device_key.to_vec(), h("0520adad5e0142aa3e325087b4ec16d8"));
    }

    #[tokio::test]
    async fn exchange_errors_on_wrong_pdu() {
        struct Bad {
            sent: Vec<Vec<u8>>,
        }
        impl ProvisioningBearer for Bad {
            async fn send(&mut self, pdu: &[u8]) -> Result<(), String> {
                self.sent.push(pdu.to_vec());
                Ok(())
            }
            async fn receive(&mut self) -> Result<(u8, Vec<u8>), String> {
                // Answer the Invite with something that is not Capabilities.
                Ok((PDU_FAILED, vec![0x03]))
            }
        }
        let mut session = ProvisioningSession::new([0u8; 16], 0, 0, 2);
        let mut bad = Bad { sent: vec![] };
        let err = session.exchange_with(&mut bad, [0u8; 16]).await.unwrap_err();
        assert!(err.contains("Capabilities"), "got: {err}");
    }
}
