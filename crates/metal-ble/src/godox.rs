//! Godox vendor command builders — the pure byte-producing half of the link.
//!
//! Every command is a fixed 8-byte **V2 frame** wrapped in the vendor opcode
//! `0x0211F0` (wire bytes `f0 11 02`, little-endian):
//!
//! ```text
//!   V2: [sub-command] [d0..d4] [end byte] [CRC8 over 0..6]
//! ```
//!
//! The functions here are pure and mirror the MIT reference `protocol.py`
//! byte-for-byte; the tests pin them to outputs computed with the reference
//! builders, so a change that alters the wire bytes fails without a light.
//!
//! Only three commands are needed for the four light kinds in the plan:
//!
//! * [`level`] — brightness only; the SL60II D is fixed-daylight, so a level
//!   command is a `0xF0` CCT command at 5600 K (which is exactly what the
//!   reference's `set_params` sends when no CCT is given).
//! * [`cct`] — brightness + colour temperature (SL60II Bi; 2800–6500 K).
//! * [`hsi`] — brightness + hue + saturation (full-RGB Godox).
//!
//! [`encode`] maps the crate's `LightState` onto those builders: HSI wins over
//! CCT wins over level, so a caller can leave fields `None`.
//!
//! Kelvin is encoded as hundreds (one byte), hue as little-endian degrees, and
//! brightness as a whole percent in data byte 0 plus its tenth in the V2 end
//! byte (`split_brightness`).

use crate::LightState;

/// Vendor company id (Telink Semiconductor).
pub const COMPANY_ID: u16 = 0x0211;
/// Vendor model id.
pub const VENDOR_MODEL_ID: u16 = 0x0000;
/// Vendor request opcode, sent little-endian as `f0 11 02`.
pub const REQUEST_OPCODE: u32 = 0x0002_11F0;
/// Vendor response opcode (status replies), `f1 11 02`.
pub const RESPONSE_OPCODE: u32 = 0x0002_11F1;

/// Sub-command: brightness + colour temperature (+ tint).
const SUB_SET_CCT: u8 = 0xF0;
/// Sub-command: hue / saturation / intensity.
const SUB_HSI: u8 = 0xF1;
/// Padding byte used in unused data slots and as the brightness end byte.
const PAD: u8 = 0xFF;

/// Protocol-wide CCT floor (what the wire can encode), not a model's range.
pub const CCT_MIN_KELVIN: u32 = 100;
/// Protocol-wide CCT ceiling (one byte of hundreds of Kelvin).
pub const CCT_MAX_KELVIN: u32 = 25_500;

/// Godox CRC-8 table (`crc = TABLE[crc ^ byte]`), lifted from the MIT ftw.
const CRC8_TABLE: [u8; 256] = [
    0, 94, 188, 226, 97, 63, 221, 131, 194, 156, 126, 32, 163, 253, 31, 65, 157, 195, 33, 127, 252,
    162, 64, 30, 95, 1, 227, 189, 62, 96, 130, 220, 35, 125, 159, 193, 66, 28, 254, 160, 225, 191,
    93, 3, 128, 222, 60, 98, 190, 224, 2, 92, 223, 129, 99, 61, 124, 34, 192, 158, 29, 67, 161,
    255, 70, 24, 250, 164, 39, 121, 155, 197, 132, 218, 56, 102, 229, 187, 89, 7, 219, 133, 103,
    57, 186, 228, 6, 88, 25, 71, 165, 251, 120, 38, 196, 154, 101, 59, 217, 135, 4, 90, 184, 230,
    167, 249, 27, 69, 198, 152, 122, 36, 248, 166, 68, 26, 153, 199, 37, 123, 58, 100, 134, 216,
    91, 5, 231, 185, 140, 210, 48, 110, 237, 179, 81, 15, 78, 16, 242, 172, 47, 113, 147, 205, 17,
    79, 173, 243, 112, 46, 204, 146, 211, 141, 111, 49, 178, 236, 14, 80, 175, 241, 19, 77, 206,
    144, 114, 44, 109, 51, 209, 143, 12, 82, 176, 238, 50, 108, 142, 208, 83, 13, 239, 177, 240,
    174, 76, 18, 145, 207, 45, 115, 202, 148, 118, 40, 171, 245, 23, 73, 8, 86, 180, 234, 105, 55,
    213, 139, 87, 9, 235, 181, 54, 104, 138, 212, 149, 203, 41, 119, 244, 170, 72, 22, 233, 183,
    85, 11, 136, 214, 52, 106, 43, 117, 151, 201, 74, 20, 246, 168, 116, 42, 200, 150, 21, 75,
    169, 247, 182, 232, 10, 84, 215, 137, 107, 53,
];

/// Godox CRC-8 over a V2 frame body.
pub fn checksum(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc = CRC8_TABLE[(crc ^ b) as usize];
    }
    crc
}

/// Encode the 24-bit vendor opcode as the three little-endian wire bytes.
pub fn encode_vendor_opcode(opcode: u32) -> [u8; 3] {
    [
        (opcode & 0xff) as u8,
        ((opcode >> 8) & 0xff) as u8,
        ((opcode >> 16) & 0xff) as u8,
    ]
}

/// Build a full Access Layer payload: vendor opcode followed by a V2 frame.
pub fn access_payload(v2_frame: &[u8]) -> Vec<u8> {
    let mut out = encode_vendor_opcode(REQUEST_OPCODE).to_vec();
    out.extend_from_slice(v2_frame);
    out
}

/// Split a 0–100 brightness into `(whole percent, tenths 0..9)`.
///
/// Out-of-range brightness is clamped rather than rejected: a show pushes
/// floats and a value of 1.0000001 should not drop the frame.
pub fn split_brightness(brightness: f32) -> (u8, u8) {
    let clamped = brightness.clamp(0.0, 100.0);
    let percent = clamped.trunc() as u8;
    let tenths = (((clamped - percent as f32) * 10.0).round() as i32).clamp(0, 9) as u8;
    (percent, tenths)
}

/// Pad `data` to five bytes with `0xFF`, append `end_byte`, then the CRC.
fn build_v2(model: u8, end_byte: u8, data: &[u8]) -> Vec<u8> {
    debug_assert!(data.len() <= 5, "V2 data must be at most 5 bytes");
    let mut padded = data.to_vec();
    padded.resize(5, PAD);
    let mut frame = Vec::with_capacity(8);
    frame.push(model);
    frame.extend_from_slice(&padded);
    frame.push(end_byte);
    frame.push(checksum(&frame));
    frame
}

/// Brightness + colour-temperature command (sub-command `0xF0`).
///
/// `kelvin` is encoded as hundreds, clamped to the protocol's representable
/// range; callers that know the model's real range should clamp first.
pub fn cct(brightness: f32, kelvin: u32) -> Vec<u8> {
    let (percent, tenths) = split_brightness(brightness);
    let k = kelvin.clamp(CCT_MIN_KELVIN, CCT_MAX_KELVIN) / 100;
    // Byte 2 is the legacy unsigned tint (centred on 50); byte 4 the signed
    // tint, zero for a model without tint control — the reference's default.
    build_v2(SUB_SET_CCT, tenths, &[percent, (k & 0xff) as u8, 50, 0, 0])
}

/// Brightness-only command for a fixed-daylight light: a `0xF0` frame at 5600 K.
pub fn level(brightness: f32) -> Vec<u8> {
    cct(brightness, 5600)
}

/// HSI command (sub-command `0xF1`): brightness, hue degrees little-endian,
/// saturation percent.
pub fn hsi(brightness: f32, hue_deg: f32, sat: f32) -> Vec<u8> {
    let (percent, tenths) = split_brightness(brightness);
    let hue = (hue_deg.rem_euclid(360.0).round() as u32) & 0xffff;
    let s = (sat.clamp(0.0, 1.0) * 100.0).round() as u8;
    build_v2(
        SUB_HSI,
        tenths,
        &[percent, (hue & 0xff) as u8, ((hue >> 8) & 0xff) as u8, s, 0],
    )
}

/// Map a [`LightState`] onto an Access Layer payload.
///
/// HSI wins over CCT wins over level; a state with `hsi` set ignores
/// `cct_kelvin`, which is what the plan's profile mapping expects.
pub fn encode(state: &LightState) -> Vec<u8> {
    let level_pct = state.level.clamp(0.0, 1.0) * 100.0;
    let frame = if let Some(h) = state.hsi {
        hsi(level_pct, h.hue_deg, h.sat)
    } else if let Some(k) = state.cct_kelvin {
        cct(level_pct, k.round().max(0.0) as u32)
    } else {
        level(level_pct)
    };
    access_payload(&frame)
}

/// Config Server **App Key Add** access payload (device-key encrypted).
///
/// `packed = NetKeyIndex | (AppKeyIndex << 12)`, little-endian, then the key.
pub fn config_app_key_add(net_key_index: u16, app_key_index: u16, app_key: &[u8; 16]) -> Vec<u8> {
    let packed = (net_key_index & 0x0fff) as u32 | (((app_key_index & 0x0fff) as u32) << 12);
    let idx = [
        (packed & 0xff) as u8,
        ((packed >> 8) & 0xff) as u8,
        ((packed >> 16) & 0xff) as u8,
    ];
    let mut out = vec![0x00];
    out.extend_from_slice(&idx);
    out.extend_from_slice(app_key);
    out
}

/// Config Server **Model App Bind** access payload for the Telink vendor model.
pub fn config_model_app_bind(
    element_address: u16,
    app_key_index: u16,
    company_id: u16,
    model_id: u16,
) -> Vec<u8> {
    let mut out = vec![0x80, 0x3D];
    out.extend_from_slice(&element_address.to_le_bytes());
    out.extend_from_slice(&app_key_index.to_le_bytes());
    out.extend_from_slice(&company_id.to_le_bytes());
    out.extend_from_slice(&model_id.to_le_bytes());
    out
}

// ---------------------------------------------------------------------------
// Tests — byte-exact against the MIT reference builders
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Hsi;

    fn hex(v: &[u8]) -> String {
        v.iter().map(|b| format!("{b:02x}")).collect()
    }
    fn unhex(s: &str) -> Vec<u8> {
        hex::decode(s).unwrap()
    }

    #[test]
    fn crc8_matches_reference() {
        assert_eq!(checksum(&unhex("fe00ffffffffff")), 127);
    }

    #[test]
    fn split_brightness_matches_reference() {
        assert_eq!(split_brightness(42.7), (42, 7));
        assert_eq!(split_brightness(100.0), (100, 0));
        assert_eq!(split_brightness(0.0), (0, 0));
        // Out of range clamps rather than panics.
        assert_eq!(split_brightness(-5.0), (0, 0));
        assert_eq!(split_brightness(250.0), (100, 0));
    }

    #[test]
    fn cct_byte_exact() {
        // Computed by running the reference build_cct_command once.
        assert_eq!(hex(&cct(75.0, 5600)), "f04b38320000003c");
        assert_eq!(hex(&cct(0.5, 4300)), "f0002b32000005bc");
        assert_eq!(hex(&cct(1.0, 2800)), "f0011c3200000061");
        assert_eq!(hex(&cct(1.0, 6500)), "f001413200000018");
    }

    #[test]
    fn level_is_cct_at_5600k() {
        assert_eq!(hex(&level(75.0)), hex(&cct(75.0, 5600)));
    }

    #[test]
    fn hsi_byte_exact() {
        assert_eq!(hex(&hsi(100.0, 240.0, 1.0)), "f164f000640000fb");
        assert_eq!(hex(&hsi(0.5, 200.0, 1.0)), "f100c80064000561");
        assert_eq!(hex(&hsi(0.0, 0.0, 0.0)), "f100000000000079");
    }

    #[test]
    fn encode_maps_state_to_the_right_command() {
        let lvl = LightState { level: 0.75, cct_kelvin: None, hsi: None };
        assert_eq!(hex(&encode(&lvl)), hex(&access_payload(&cct(75.0, 5600))));

        let warm = LightState { level: 0.5, cct_kelvin: Some(4300.0), hsi: None };
        // state.level is 0..1, so 0.5 becomes 50 percent on the wire.
        assert_eq!(hex(&encode(&warm)), hex(&access_payload(&cct(50.0, 4300))));

        let rgb = LightState {
            level: 1.0,
            cct_kelvin: Some(4300.0), // ignored: HSI wins
            hsi: Some(Hsi { hue_deg: 240.0, sat: 1.0 }),
        };
        assert_eq!(hex(&encode(&rgb)), hex(&access_payload(&hsi(100.0, 240.0, 1.0))));
    }

    #[test]
    fn vendor_opcode_is_little_endian_f01102() {
        assert_eq!(encode_vendor_opcode(REQUEST_OPCODE), [0xf0, 0x11, 0x02]);
        assert_eq!(encode_vendor_opcode(135_664), [0xf0, 0x11, 0x02]);
        assert_eq!(encode_vendor_opcode(RESPONSE_OPCODE), [0xf1, 0x11, 0x02]);
    }

    #[test]
    fn access_payload_prefixes_opcode() {
        let p = access_payload(&[0xAA, 0xBB]);
        assert_eq!(p, vec![0xf0, 0x11, 0x02, 0xAA, 0xBB]);
    }

    #[test]
    fn config_builders_byte_exact() {
        let key: [u8; 16] = (0u8..16).collect::<Vec<_>>().try_into().unwrap();
        assert_eq!(
            hex(&config_app_key_add(0, 0, &key)),
            "00000000000102030405060708090a0b0c0d0e0f"
        );
        assert_eq!(
            hex(&config_model_app_bind(2, 0, COMPANY_ID, VENDOR_MODEL_ID)),
            "803d0200000011020000"
        );
    }
}
