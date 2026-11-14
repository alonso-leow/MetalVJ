//! Bluetooth Mesh cryptography (Mesh Profile §3.8.2 / §3.8.4).
//!
//! Everything the link layer needs is in this module and nowhere else, so the
//! spec sample data can be checked without a radio attached:
//!
//! * `s1` — the SALT generation function used by every key derivation and by
//!   provisioning (`ConfirmationSalt = s1(ConfirmationInputs)`).
//! * `k1` — the general key-derivation function (`prsk`/`prsn`/`prdk`/`prck`
//!   for provisioning, `id128` for the identity/beacon keys).
//! * `k2` — NetKey → (NID, EncryptionKey, PrivacyKey). The second return value
//!   is the *Network* EncryptionKey used for the network-layer AES-CCM, and the
//!   third is what obfuscates the six-byte network header.
//! * `k3` — NetKey → 64-bit Network ID (the beacon's network identifier).
//! * `k4` — AppKey → 6-bit AID, placed in the AKF byte of an access PDU.
//! * `ccm_encrypt`/`ccm_decrypt` — AES-CCM with a 13-byte nonce and a 4- or
//!   8-byte MIC, the only AEAD the mesh uses.
//! * `obfuscate` — the header obfuscation (AES-ECB of a PrivacyRandom-derived
//!   block; the transform is its own inverse).
//!
//! The module deliberately reimplements CCM/CMAC through RustCrypto's primitives
//! rather than pulling a mesh crate: the header obfuscation and the exact nonce
//! layouts are mesh-specific and easier to audit against §8 than to bind.
//!
//! Test vectors: Mesh Profile §8.1 (security sample data), §8.2 (key derivation),
//! §8.3 (mesh message sample data) and §8.17 (provisioning sample data) are
//! asserted below. They are the gate the plan requires to pass before the radio
//! is touched.

use aes::cipher::{BlockEncrypt, KeyInit as BlockKeyInit};
use aes::Aes128;
use ccm::aead::{Aead, AeadInPlace};
use ccm::consts::{U13, U4, U8};
use ccm::Ccm;
use cmac::{Cmac, Mac};

/// AES-CCM with a 13-byte nonce and a 4-byte MIC (upper transport, access PDU).
pub type AesCcm4 = Ccm<Aes128, U4, U13>;
/// AES-CCM with a 13-byte nonce and an 8-byte MIC (provisioning Data PDU).
pub type AesCcm8 = Ccm<Aes128, U8, U13>;

/// AES-CMAC. `key` is 16 bytes; `msg` is arbitrary length.
pub fn aes_cmac(key: &[u8; 16], msg: &[u8]) -> [u8; 16] {
    let mut mac = <Cmac<Aes128> as Mac>::new_from_slice(key).expect("AES-128 key is 16 bytes");
    Mac::update(&mut mac, msg);
    let out = mac.finalize().into_bytes();
    let mut r = [0u8; 16];
    r.copy_from_slice(&out);
    r
}

/// `s1(M) = AES-CMAC(key = 16 zero bytes, M)` (Mesh Profile §3.8.2.1).
pub fn s1(m: &[u8]) -> [u8; 16] {
    aes_cmac(&[0u8; 16], m)
}

/// `k1(N, SALT, P)`: T = CMAC(SALT, N); k1 = CMAC(T, P) (Mesh Profile §3.8.2.2).
pub fn k1(n: &[u8], salt: &[u8; 16], p: &[u8]) -> [u8; 16] {
    let t = aes_cmac(salt, n);
    aes_cmac(&t, p)
}

/// `k2(N, P)` → `(NID, EncryptionKey, PrivacyKey)` (Mesh Profile §3.8.2.3).
///
/// `n` is the NetKey; `p` is the single parameter byte (0x00 for managed
/// flooding, 0x01 for friendship, 0x02 for directed forwarding).
pub fn k2(n: &[u8; 16], p: u8) -> (u8, [u8; 16], [u8; 16]) {
    let salt = aes_cmac(&[0u8; 16], b"smk2");
    let t = aes_cmac(&salt, n);

    let t1 = aes_cmac(&t, &[p, 0x01]);

    let mut m2 = [0u8; 18];
    m2[..16].copy_from_slice(&t1);
    m2[16] = p;
    m2[17] = 0x02;
    let t2 = aes_cmac(&t, &m2);

    let mut m3 = [0u8; 18];
    m3[..16].copy_from_slice(&t2);
    m3[16] = p;
    m3[17] = 0x03;
    let t3 = aes_cmac(&t, &m3);

    (t1[15] & 0x7f, t2, t3)
}

/// `k2` with a variable-length parameter block (friendship / directed) — the
/// spec feeds `P = LPNAddress||FriendAddress||LPNCounter||FriendCounter` etc.
pub fn k2_with_params(n: &[u8; 16], p: &[u8]) -> (u8, [u8; 16], [u8; 16]) {
    let salt = aes_cmac(&[0u8; 16], b"smk2");
    let t = aes_cmac(&salt, n);

    let mut m1 = p.to_vec();
    m1.push(0x01);
    let t1 = aes_cmac(&t, &m1);

    let mut m2 = t1.to_vec();
    m2.extend_from_slice(p);
    m2.push(0x02);
    let t2 = aes_cmac(&t, &m2);

    let mut m3 = t2.to_vec();
    m3.extend_from_slice(p);
    m3.push(0x03);
    let t3 = aes_cmac(&t, &m3);

    (t1[15] & 0x7f, t2, t3)
}

/// `k3(N)` → 64-bit Network ID (Mesh Profile §3.8.2.4).
pub fn k3(n: &[u8; 16]) -> [u8; 8] {
    let salt = aes_cmac(&[0u8; 16], b"smk3");
    let t = aes_cmac(&salt, n);
    let t2 = aes_cmac(&t, b"id64\x01");
    let mut out = [0u8; 8];
    out.copy_from_slice(&t2[8..]);
    out
}

/// `k4(AppKey)` → 6-bit AID (Mesh Profile §3.8.2.5).
pub fn k4(app_key: &[u8; 16]) -> u8 {
    let salt = aes_cmac(&[0u8; 16], b"smk4");
    let t = aes_cmac(&salt, app_key);
    let t2 = aes_cmac(&t, b"id6\x01");
    t2[15] & 0x3f
}

/// AES-CCM encryption, 13-byte nonce, 4- or 8-byte MIC.
///
/// Returns `ciphertext || MIC`. Panics if `mic_len` is neither 4 nor 8 — the
/// mesh uses only those two sizes, and a silent fallback would hide a bug.
pub fn ccm_encrypt(key: &[u8; 16], nonce13: &[u8; 13], data: &[u8], mic_len: usize) -> Vec<u8> {
    match mic_len {
        4 => AesCcm4::new(key.into())
            .encrypt(nonce13.into(), data)
            .expect("CCM encryption cannot fail for valid sizes"),
        8 => AesCcm8::new(key.into())
            .encrypt(nonce13.into(), data)
            .expect("CCM encryption cannot fail for valid sizes"),
        other => panic!("unsupported MIC length {other} (mesh uses 4 or 8)"),
    }
}

/// AES-CCM decryption; returns the plaintext or `None` if the MIC fails.
pub fn ccm_decrypt(
    key: &[u8; 16],
    nonce13: &[u8; 13],
    data: &[u8],
    mic_len: usize,
) -> Option<Vec<u8>> {
    match mic_len {
        4 => AesCcm4::new(key.into()).decrypt(nonce13.into(), data).ok(),
        8 => AesCcm8::new(key.into()).decrypt(nonce13.into(), data).ok(),
        _ => None,
    }
}

/// AES-CCM in-place decrypt of a slice whose last `mic_len` bytes are the MIC.
/// Used by transport code that wants to avoid re-allocating the ciphertext.
pub fn ccm_decrypt_in_place(
    key: &[u8; 16],
    nonce13: &[u8; 13],
    buf: &mut Vec<u8>,
    mic_len: usize,
) -> Option<()> {
    match mic_len {
        4 => AesCcm4::new(key.into())
            .decrypt_in_place(nonce13.into(), &[], buf)
            .ok(),
        8 => AesCcm8::new(key.into())
            .decrypt_in_place(nonce13.into(), &[], buf)
            .ok(),
        _ => None,
    }
}

/// Header obfuscation (Mesh Profile §3.8.4). XORs network-PDU bytes 1..6 with
/// the first six bytes of the AES-ECB encryption of
/// `0000000000 || IVIndex || PrivacyRandom`, where PrivacyRandom is bytes 7..14
/// of the (already-encrypted) PDU. The transform is its own inverse.
pub fn obfuscate(privacy_key: &[u8; 16], iv_index: u32, net_pdu: &[u8]) -> Vec<u8> {
    assert!(
        net_pdu.len() >= 14,
        "network PDU must include the 7-byte PrivacyRandom at offset 7"
    );
    let mut plaintext = [0u8; 16];
    // bytes 0..5 stay zero; 5..9 = IVIndex; 9..16 = PrivacyRandom
    plaintext[5..9].copy_from_slice(&iv_index.to_be_bytes());
    plaintext[9..16].copy_from_slice(&net_pdu[7..14]);

    let cipher = Aes128::new(privacy_key.into());
    let mut block = aes::cipher::Block::<Aes128>::clone_from_slice(&plaintext);
    cipher.encrypt_block(&mut block);

    let mut out = net_pdu.to_vec();
    for i in 1..7 {
        out[i] = net_pdu[i] ^ block[i - 1];
    }
    out
}

// ---------------------------------------------------------------------------
// Tests — Mesh Profile sample data
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> Vec<u8> {
        hex::decode(s).expect("valid hex")
    }
    fn a16(s: &str) -> [u8; 16] {
        h(s).try_into().unwrap()
    }

    // ---- §8.1 Security sample data ----------------------------------------
    // AppKey : 3216d1509884b533248541792b877f98
    // NetKey : f7a2a44f8e8a8029064f173ddc1e2b00

    #[test]
    fn spec_8_1_s1() {
        assert_eq!(s1(b"test").to_vec(), h("b73cefbd641ef2ea598c2b6efb62f79c"));
    }

    #[test]
    fn spec_8_1_k1() {
        let n = h("3216d1509884b533248541792b877f98");
        let salt = a16("2ba14ffa0df84a2831938d57d276cab4");
        let p = h("5a09d60797eeb4478aada59db3352a0d");
        // k1 T (intermediate) is also given in the spec.
        assert_eq!(aes_cmac(&salt, &n).to_vec(), h("c764bea25cf9738b08956ea3c712d5af"));
        assert_eq!(k1(&n, &salt, &p).to_vec(), h("f6ed15a8934afbe7d83e8dcb57fcf5d7"));
    }

    #[test]
    fn spec_8_1_k2_managed_flooding() {
        let n = a16("f7a2a44f8e8a8029064f173ddc1e2b00");
        let (nid, enc, priv_k) = k2(&n, 0x00);
        assert_eq!(nid, 0x7f);
        assert_eq!(enc.to_vec(), h("9f589181a0f50de73c8070c7a6d27f46"));
        assert_eq!(priv_k.to_vec(), h("4c715bd4a64b938f99b453351653124f"));
    }

    #[test]
    fn spec_8_1_k2_friendship() {
        let n = a16("f7a2a44f8e8a8029064f173ddc1e2b00");
        let p = h("010203040506070809");
        let (nid, enc, priv_k) = k2_with_params(&n, &p);
        assert_eq!(nid, 0x73);
        assert_eq!(enc.to_vec(), h("11efec0642774992510fb5929646df49"));
        assert_eq!(priv_k.to_vec(), h("d4d7cc0dfa772d836a8df9df5510d7a7"));
    }

    #[test]
    fn spec_8_1_k3() {
        let n = a16("f7a2a44f8e8a8029064f173ddc1e2b00");
        assert_eq!(k3(&n).to_vec(), h("ff046958233db014"));
    }

    #[test]
    fn spec_8_1_k4() {
        let app = a16("3216d1509884b533248541792b877f98");
        assert_eq!(k4(&app), 0x38);
    }

    // ---- §8.2 Mesh key derivation sample data -----------------------------
    // AppKey 63964771734fbd76e3b40519d1d94a48 / NetKey 7dd7364cd842ad18c17c2b820c84c3d6

    #[test]
    fn spec_8_2_k4_and_k3() {
        let app = a16("63964771734fbd76e3b40519d1d94a48");
        assert_eq!(k4(&app), 0x26);
        let n = a16("7dd7364cd842ad18c17c2b820c84c3d6");
        assert_eq!(k3(&n).to_vec(), h("3ecaff672f673370"));
    }

    #[test]
    fn spec_8_2_k2_directed() {
        let n = a16("7dd7364cd842ad18c17c2b820c84c3d6");
        let (nid, enc, priv_k) = k2(&n, 0x02);
        assert_eq!(nid, 0x0d);
        assert_eq!(enc.to_vec(), h("b47a02c6cc9b4ac4cb9b88e765c9ade4"));
        assert_eq!(priv_k.to_vec(), h("9bf7ab5a5ad415fbd77e07bb808f4865"));
    }

    #[test]
    fn spec_8_2_identity_and_beacon_keys() {
        // IdentityKey = k1(N, SALT, "id128"||0x01)
        let n = h("7dd7364cd842ad18c17c2b820c84c3d6");
        let salt = a16("f8795a1aabf182e4f163d86e245e19f4");
        assert_eq!(k1(&n, &salt, b"id128\x01").to_vec(), h("84396c435ac48560b5965385253e210c"));
        let salt = a16("2c24619ab793c1233f6e226738393dec");
        assert_eq!(k1(&n, &salt, b"id128\x01").to_vec(), h("5423d967da639a99cb02231a83f7d254"));
    }

    // ---- §8.3 Mesh message sample data ------------------------------------
    // NetworkPDU encryption + obfuscation. NetKey 7dd7364c..., IVIndex 12345678.

    fn network_pdu(ctl_ttl: u8, seq: u32, src: u16, dst: u16, lower: &[u8], mic: usize) -> Vec<u8> {
        let net_key = a16("7dd7364cd842ad18c17c2b820c84c3d6");
        let (nid, enc_key, priv_key) = k2(&net_key, 0x00);
        let iv_index = 0x1234_5678u32;

        let mut nonce = [0u8; 13];
        nonce[0] = 0x00; // network nonce type
        nonce[1] = ctl_ttl;
        nonce[2..5].copy_from_slice(&seq.to_be_bytes()[1..]); // 24-bit SEQ
        nonce[5..7].copy_from_slice(&src.to_be_bytes());
        // 7..9 already zero (DST)
        nonce[9..13].copy_from_slice(&iv_index.to_be_bytes());

        let mut plain = Vec::with_capacity(2 + lower.len());
        plain.extend_from_slice(&dst.to_be_bytes());
        plain.extend_from_slice(lower);
        let enc = ccm_encrypt(&enc_key, &nonce, &plain, mic);

        let mut pdu = Vec::with_capacity(7 + enc.len());
        pdu.push(((iv_index as u8 & 1) << 7) | nid);
        pdu.push(ctl_ttl);
        pdu.extend_from_slice(&seq.to_be_bytes()[1..]);
        pdu.extend_from_slice(&src.to_be_bytes());
        pdu.extend_from_slice(&enc);
        obfuscate(&priv_key, iv_index, &pdu)
    }

    #[test]
    fn spec_8_3_message_1() {
        let got = network_pdu(0x80, 0x000001, 0x1201, 0xfffd, &h("034b50057e400000010000"), 8);
        assert_eq!(got, h("68eca487516765b5e5bfdacbaf6cb7fb6bff871f035444ce83a670df"));
    }

    #[test]
    fn spec_8_3_message_2_and_3() {
        let m2 = network_pdu(0x80, 0x014820, 0x2345, 0x1201, &h("04320308ba072f"), 8);
        assert_eq!(m2, h("68d4c826296d7979d7dbc0c9b4d43eebec129d20a620d01e"));
        let m3 = network_pdu(0x80, 0x2b3832, 0x2fe3, 0x1201, &h("04fa0205a6000a"), 8);
        assert_eq!(m3, h("68da062bc96df253273086b8c5ee00bdd9cfcc62a2ddf572"));
    }

    #[test]
    fn spec_8_3_message_6_and_7() {
        let m6a = network_pdu(
            0x04,
            0x3129ab,
            0x0003,
            0x1201,
            &h("8026ac01ee9dddfd2169326d23f3afdf"),
            4,
        );
        assert_eq!(m6a, h("68cab5c5348a230afba8c63d4e686364979deaf4fd40961145939cda0e"));
        let m6b = network_pdu(
            0x04,
            0x3129ac,
            0x0003,
            0x1201,
            &h("8026ac21cfdc18c52fdef772e0e17308"),
            4,
        );
        assert_eq!(m6b, h("681615b5dd4a846cae0c032bf0746f44f1b8cc8ce5edc57e55beed49c0"));
        let m7 = network_pdu(0x8b, 0x014835, 0x2345, 0x0003, &h("00a6ac00000002"), 8);
        assert_eq!(m7, h("68e476b5579c980d0d730f94d7f3509df987bb417eb7c05f"));
    }

    #[test]
    fn obfuscate_is_its_own_inverse() {
        let key = a16("8b84eedec100067d670971dd2aa700cf");
        let pdu = h("68eca487516765b5e5bfdacbaf6cb7fb6bff871f035444ce83a670df");
        let round = obfuscate(&key, 0x1234_5678, &obfuscate(&key, 0x1234_5678, &pdu));
        assert_eq!(round, pdu);
    }

    // ---- §8.17 Provisioning sample data -----------------------------------
    // BTM_ECDH_P256_CMAC_AES128_AES_CCM

    #[test]
    fn spec_8_17_confirmation_and_session_keys() {
        let secret = h("ab85843a2f6d883f62e5684b38e307335fe6e1945ecd19604105c6f23221eb69");
        let conf_salt = a16("5faabe187337c71cc6c973369dcaa79a");
        let conf_key = k1(&secret, &conf_salt, b"prck");
        assert_eq!(conf_key.to_vec(), h("e31fe046c68ec339c425fc6629f0336f"));

        let rp = h("8b19ac31d58b124c946209b5db1021b9");
        let mut input = rp.clone();
        input.extend_from_slice(&[0u8; 16]); // AuthValue is zero (No-OOB)
        assert_eq!(aes_cmac(&conf_key, &input).to_vec(), h("b38a114dfdca1fe153bd2c1e0dc46ac2"));

        let rd = h("55a2a2bca04cd32ff6f346bd0a0c1a3a");
        let mut input = rd.clone();
        input.extend_from_slice(&[0u8; 16]);
        assert_eq!(aes_cmac(&conf_key, &input).to_vec(), h("eeba521c196b52cc2e37aa40329f554e"));

        // ProvisioningSalt = s1(ConfirmationSalt || RandomProvisioner || RandomDevice)
        let mut ps_in = conf_salt.to_vec();
        ps_in.extend_from_slice(&rp);
        ps_in.extend_from_slice(&rd);
        let prov_salt = s1(&ps_in);
        assert_eq!(prov_salt.to_vec(), h("a21c7d45f201cf9489a2fb57145015b4"));

        assert_eq!(k1(&secret, &prov_salt, b"prsk").to_vec(), h("c80253af86b33dfa450bbdb2a191fea3"));
        let nonce_full = k1(&secret, &prov_salt, b"prsn");
        assert_eq!(nonce_full[3..].to_vec(), h("da7ddbe78b5f62b81d6847487e"));
        assert_eq!(k1(&secret, &prov_salt, b"prdk").to_vec(), h("0520adad5e0142aa3e325087b4ec16d8"));
    }

    #[test]
    fn spec_8_17_data_encryption() {
        // Data plaintext: NetKey||NetKeyIndex||Flags||IVIndex||UnicastAddress
        let data = h("efb2255e6422d330088e09bb015ed707056700010203040b0c");
        let session_key = a16("c80253af86b33dfa450bbdb2a191fea3");
        let mut nonce = [0u8; 13];
        nonce.copy_from_slice(&h("da7ddbe78b5f62b81d6847487e"));
        let got = ccm_encrypt(&session_key, &nonce, &data, 8);
        assert_eq!(
            got,
            h("d0bd7f4a89a2ff6222af59a90a60ad58acfe3123356f5cec2973e0ec50783b10c7")
        );
        // And the MIC splits as the spec shows: 33 bytes total, 8-byte MIC.
        assert_eq!(&got[25..], &h("73e0ec50783b10c7")[..]);
    }

    #[test]
    fn ccm_round_trips() {
        let key = a16("000102030405060708090a0b0c0d0e0f");
        let nonce = [0u8; 13];
        for mic in [4usize, 8] {
            let ct = ccm_encrypt(&key, &nonce, b"hello mesh", mic);
            assert_eq!(ct.len(), 10 + mic);
            assert_eq!(ccm_decrypt(&key, &nonce, &ct, mic).as_deref(), Some(&b"hello mesh"[..]));
            // A flipped bit must fail authentication.
            let mut bad = ct.clone();
            bad[0] ^= 1;
            assert!(ccm_decrypt(&key, &nonce, &bad, mic).is_none());
        }
    }
}
