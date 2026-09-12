// SPDX-License-Identifier: GPL-3.0-or-later
// AirPods packet formats ported from the bundled LibrePods/Omapods backend.

use aes::Aes128;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const HANDSHAKE: &[u8] = &[0, 0, 4, 0, 1, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0];
pub const FEATURES: &[u8] = &[4, 0, 4, 0, 0x4d, 0, 0xd7, 0, 0, 0, 0, 0, 0, 0];
pub const NOTIFICATIONS: &[u8] = &[4, 0, 4, 0, 0x0f, 0, 0xff, 0xff, 0xff, 0xff, 0xff];
pub const REQUEST_MAGIC_KEYS: &[u8] = &[4, 0, 4, 0, 0x30, 0, 5, 0];
const BATTERY_HEADER: &[u8] = &[4, 0, 4, 0, 4, 0];
const EAR_HEADER: &[u8] = &[4, 0, 4, 0, 6, 0];
const CONTROL_HEADER: &[u8] = &[4, 0, 4, 0, 9, 0];
const METADATA_HEADER: &[u8] = &[4, 0, 4, 0, 0x1d];
const MAGIC_KEYS_HEADER: &[u8] = &[4, 0, 4, 0, 0x31, 0, 2];
const CONVERSATION_HEADER: &[u8] = &[4, 0, 4, 0, 0x4b, 0, 2, 0, 1];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Component {
    Headset,
    #[default]
    Left,
    Right,
    Case,
}

impl Component {
    fn from_wire(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Headset),
            2 => Some(Self::Right),
            4 => Some(Self::Left),
            8 => Some(Self::Case),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EarState {
    InEar,
    Out,
    InCase,
    #[default]
    Disconnected,
}

impl EarState {
    fn from_wire(value: u8) -> Self {
        match value {
            0 => Self::InEar,
            1 => Self::Out,
            2 => Self::InCase,
            _ => Self::Disconnected,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComponentReading {
    pub component: Component,
    pub level: u8,
    pub charging: bool,
    pub available: bool,
}

// Deliberately redact keys if an event is printed in a diagnostic.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MagicKeys {
    pub irk: [u8; 16],
    pub key: [u8; 16],
}

impl fmt::Debug for MagicKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MagicKeys(<redacted>)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    HandshakeAck,
    FeaturesAck,
    Disconnected,
    Battery(Vec<ComponentReading>),
    Ear {
        primary: EarState,
        secondary: EarState,
    },
    NoiseMode(i32),
    AdaptiveLevel(u8),
    Conversation(bool),
    ConversationActivity(u8),
    Metadata {
        name: String,
        model_number: String,
    },
    MagicKeys(MagicKeys),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseError(pub &'static str);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ParseError {}

pub fn control_packet(id: u8, value: u8) -> [u8; 11] {
    [4, 0, 4, 0, 9, 0, id, value, 0, 0, 0]
}

pub fn noise_packet(mode: i32) -> Option<[u8; 11]> {
    (0..=3)
        .contains(&mode)
        .then(|| control_packet(0x0d, (mode + 1) as u8))
}

pub fn conversation_packet(enabled: bool) -> [u8; 11] {
    control_packet(0x28, if enabled { 1 } else { 2 })
}

pub fn adaptive_packet(level: u8) -> Option<[u8; 11]> {
    (level <= 100).then(|| control_packet(0x2e, level))
}

/// Parse one L2CAP datagram. Unknown packets are ignored; malformed known
/// packets cannot partially update state because parsing has no side effects.
pub fn parse_packet(data: &[u8]) -> Result<Option<Event>, ParseError> {
    if data.len() > 65535 {
        return Err(ParseError("oversized packet"));
    }
    let event = if data.starts_with(&[1, 0, 4, 0]) {
        Event::HandshakeAck
    } else if data.starts_with(&[4, 0, 4, 0, 0x2b, 0]) {
        Event::FeaturesAck
    } else if data == [0, 1, 0, 0] {
        Event::Disconnected
    } else if data.starts_with(BATTERY_HEADER) {
        Event::Battery(parse_battery(data)?)
    } else if data.starts_with(EAR_HEADER) {
        if data.len() != 8 {
            return Err(ParseError("invalid ear packet length"));
        }
        Event::Ear {
            primary: EarState::from_wire(data[6]),
            secondary: EarState::from_wire(data[7]),
        }
    } else if data.starts_with(CONTROL_HEADER) {
        if data.len() != 11 {
            return Err(ParseError("invalid control packet length"));
        }
        let value = data[7];
        match data[6] {
            0x0d if (1..=4).contains(&value) => Event::NoiseMode(i32::from(value) - 1),
            0x2e if value <= 100 => Event::AdaptiveLevel(value),
            0x28 if value == 1 || value == 2 => Event::Conversation(value == 1),
            0x0d | 0x2e | 0x28 => return Err(ParseError("invalid control value")),
            _ => return Ok(None),
        }
    } else if data.starts_with(CONVERSATION_HEADER) {
        if data.len() != 10 {
            return Err(ParseError("invalid conversation packet length"));
        }
        Event::ConversationActivity(data[9])
    } else if data.starts_with(METADATA_HEADER) {
        let mut fields = data.get(11..).ok_or(ParseError("truncated metadata"))?;
        let name = take_string(&mut fields)?;
        let model_number = take_string(&mut fields)?;
        // Validate the third field as well, without retaining unused branding.
        let _manufacturer = take_string(&mut fields)?;
        Event::Metadata { name, model_number }
    } else if data.starts_with(MAGIC_KEYS_HEADER) {
        // Two fixed TLVs: type, big-endian length, reserved byte, 16-byte key.
        if data.len() < 47 || data[7..10] != [1, 0, 16] || data[27..30] != [4, 0, 16] {
            return Err(ParseError("invalid magic key packet"));
        }
        Event::MagicKeys(MagicKeys {
            irk: data[11..27].try_into().expect("validated key length"),
            key: data[31..47].try_into().expect("validated key length"),
        })
    } else {
        return Ok(None);
    };
    Ok(Some(event))
}

fn take_string(data: &mut &[u8]) -> Result<String, ParseError> {
    let end = data
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(ParseError("unterminated metadata field"))?;
    if end > 255 {
        return Err(ParseError("oversized metadata field"));
    }
    let value = std::str::from_utf8(&data[..end])
        .map_err(|_| ParseError("invalid metadata text"))?
        .to_owned();
    *data = &data[end + 1..];
    Ok(value)
}

fn parse_battery(data: &[u8]) -> Result<Vec<ComponentReading>, ParseError> {
    let count = usize::from(*data.get(6).ok_or(ParseError("missing battery count"))?);
    if count > 3 || data.len() != 7 + 5 * count {
        return Err(ParseError("invalid battery packet length"));
    }
    let mut readings = Vec::with_capacity(count);
    for record in data[7..].as_chunks::<5>().0 {
        if record[1] != 1 || record[4] != 1 {
            return Err(ParseError("invalid battery record delimiters"));
        }
        let Some(component) = Component::from_wire(record[0]) else {
            continue;
        };
        if !matches!(record[3], 1 | 2 | 4) || (record[3] != 4 && record[2] > 100) {
            return Err(ParseError("invalid battery reading"));
        }
        if readings
            .iter()
            .any(|reading: &ComponentReading| reading.component == component)
        {
            return Err(ParseError("duplicate battery component"));
        }
        readings.push(ComponentReading {
            component,
            level: record[2],
            charging: record[3] == 1,
            available: record[3] != 4,
        });
    }
    Ok(readings)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advertisement {
    pub model_id: u16,
    pub primary_left: bool,
    pub pod_in_case: bool,
    pub left_in_ear: bool,
    pub right_in_ear: bool,
    pub case_level: Option<u8>,
    pub case_charging: bool,
    pub lid_state: u8,
    pub encrypted_payload: [u8; 16],
}

/// Manufacturer data for Apple's company ID 0x004c, excluding that ID.
/// The unencrypted pod percentages are deliberately not exposed: they are
/// rounded to tens and must never overwrite the independently measured values.
pub fn parse_advertisement(data: &[u8]) -> Option<Advertisement> {
    if data.len() < 27 || data[0] != 7 || data[1] != 25 || data[2] != 1 {
        return None;
    }
    let status = data[5];
    let primary_left = status & 0x20 != 0;
    let pod_in_case = status & 0x40 != 0;
    let flipped = !primary_left ^ pod_in_case;
    let case_nibble = data[7] & 0x0f;
    Some(Advertisement {
        model_id: u16::from_be_bytes([data[3], data[4]]),
        primary_left,
        pod_in_case,
        left_in_ear: status & if flipped { 0x08 } else { 0x02 } != 0,
        right_in_ear: status & if flipped { 0x02 } else { 0x08 } != 0,
        case_level: (case_nibble <= 10).then(|| case_nibble * 10),
        case_charging: data[7] & 0x40 != 0,
        lid_state: if pod_in_case { (data[8] >> 3) & 1 } else { 2 },
        encrypted_payload: data[11..27].try_into().ok()?,
    })
}

/// Verify a random Bluetooth address before accepting another device's BLE data.
/// Bluetooth ah() uses reversed key/data byte order relative to AES notation.
pub fn verify_rpa(address: &str, irk: &[u8; 16]) -> bool {
    let parts: Vec<_> = address.split(':').collect();
    if parts.len() != 6 {
        return false;
    }
    let mut bytes = [0_u8; 6];
    for (i, part) in parts.iter().enumerate() {
        if part.len() != 2 {
            return false;
        }
        let Ok(byte) = u8::from_str_radix(part, 16) else {
            return false;
        };
        bytes[5 - i] = byte;
    }
    let mut key = *irk;
    key.reverse();
    let mut plaintext = [0_u8; 16];
    plaintext[..3].copy_from_slice(&bytes[3..6]);
    plaintext.reverse();
    let cipher = Aes128::new(&key.into());
    let mut block = plaintext.into();
    cipher.encrypt_block(&mut block);
    block.reverse();
    block[..3] == bytes[..3]
}

/// A single AES-CBC block with a zero IV equals a single AES block decryption.
/// The protocol supplies no authentication tag; RPA identity must be checked
/// separately and every decoded percentage is range checked by the model.
pub fn decrypt_battery(payload: &[u8], key: &[u8; 16]) -> Option<[u8; 16]> {
    let input: [u8; 16] = payload.try_into().ok()?;
    let cipher = Aes128::new(&(*key).into());
    let mut block = input.into();
    cipher.decrypt_block(&mut block);
    Some(block.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(hex: &str) -> Vec<u8> {
        hex.as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|s| u8::from_str_radix(std::str::from_utf8(s).unwrap(), 16).unwrap())
            .collect()
    }

    #[test]
    fn battery_preserves_distinct_components() {
        let packet = bytes("040004000400030401420201020157020108011e0101");
        let Some(Event::Battery(values)) = parse_packet(&packet).unwrap() else {
            panic!("battery event");
        };
        assert_eq!(
            values
                .iter()
                .map(|r| (r.component, r.level))
                .collect::<Vec<_>>(),
            vec![
                (Component::Left, 66),
                (Component::Right, 87),
                (Component::Case, 30)
            ]
        );
    }

    #[test]
    fn truncated_and_malformed_packets_never_parse_as_complete_updates() {
        let battery = bytes("040004000400030401420201020157020108011e0101");
        for n in 0..battery.len() {
            assert!(!matches!(
                parse_packet(&battery[..n]),
                Ok(Some(Event::Battery(_)))
            ));
        }
        for index in [6, 8, 9, 10, 11] {
            let mut bad = battery.clone();
            bad[index] = 255;
            assert!(parse_packet(&bad).is_err());
        }
        assert!(parse_packet(&bytes("0400040004000204014202010401570201")).is_err());
        assert_eq!(parse_packet(&[0xff; 40]).unwrap(), None);
    }

    #[test]
    fn control_values_and_ui_mode_mapping() {
        assert_eq!(HANDSHAKE, bytes("00000400010002000000000000000000"));
        assert_eq!(FEATURES, bytes("040004004d00d700000000000000"));
        assert_eq!(NOTIFICATIONS, bytes("040004000f00ffffffffff"));
        assert_eq!(REQUEST_MAGIC_KEYS, bytes("0400040030000500"));
        for mode in 0..4 {
            let packet = noise_packet(mode).unwrap();
            assert_eq!(packet[7], mode as u8 + 1);
            assert_eq!(parse_packet(&packet).unwrap(), Some(Event::NoiseMode(mode)));
        }
        assert!(noise_packet(-1).is_none());
        assert!(noise_packet(4).is_none());
        assert_eq!(
            parse_packet(&conversation_packet(false)).unwrap(),
            Some(Event::Conversation(false))
        );
        assert_eq!(
            parse_packet(&adaptive_packet(100).unwrap()).unwrap(),
            Some(Event::AdaptiveLevel(100))
        );
        assert!(adaptive_packet(101).is_none());
        assert!(parse_packet(&control_packet(0x28, 9)).is_err());
        assert!(parse_packet(&control_packet(0x0d, 0)).is_err());
    }

    #[test]
    fn metadata_requires_complete_utf8_fields() {
        let mut packet = bytes("040004001d000000000000");
        packet.extend_from_slice(b"AirPods\0A3064\0Apple\0");
        assert_eq!(
            parse_packet(&packet).unwrap(),
            Some(Event::Metadata {
                name: "AirPods".into(),
                model_number: "A3064".into()
            })
        );
        packet.pop();
        assert!(parse_packet(&packet).is_err());
        packet.extend_from_slice(&[255, 0]);
        assert!(parse_packet(&packet).is_err());
    }

    #[test]
    fn magic_keys_require_both_complete_records_and_redact_debug() {
        let mut packet = bytes("0400040031000201001000");
        packet.extend_from_slice(&[3; 16]);
        packet.extend_from_slice(&[4, 0, 16, 0]);
        packet.extend_from_slice(&[9; 16]);
        let event = parse_packet(&packet).unwrap().unwrap();
        assert!(format!("{event:?}").contains("<redacted>"));
        assert_eq!(
            event,
            Event::MagicKeys(MagicKeys {
                irk: [3; 16],
                key: [9; 16]
            })
        );
        for n in MAGIC_KEYS_HEADER.len()..packet.len() {
            assert!(parse_packet(&packet[..n]).is_err());
        }
    }

    #[test]
    fn captured_ble_frame_and_case_unknown() {
        let packet = bytes("071901272021888f110004b48a83d66c322a4745cb15da3fd6ab2b");
        let adv = parse_advertisement(&packet).unwrap();
        assert_eq!(adv.model_id, 0x2720);
        assert_eq!(adv.case_level, None);
        assert!(adv.primary_left);
        for n in 0..packet.len() {
            assert!(parse_advertisement(&packet[..n]).is_none());
        }
        let mut docked = packet;
        docked[5] |= 0x40;
        docked[7] = 0x46;
        let adv = parse_advertisement(&docked).unwrap();
        assert_eq!(adv.case_level, Some(60));
        assert!(adv.case_charging);
        assert_eq!(adv.lid_state, 0);
    }

    #[test]
    fn single_block_decryption_matches_nist_aes_vector() {
        let key: [u8; 16] = bytes("000102030405060708090a0b0c0d0e0f")
            .try_into()
            .unwrap();
        let ciphertext = bytes("69c4e0d86a7b0430d8cdb78070b4c55a");
        assert_eq!(
            decrypt_battery(&ciphertext, &key).unwrap().to_vec(),
            bytes("00112233445566778899aabbccddeeff")
        );
        assert!(decrypt_battery(&ciphertext[..15], &key).is_none());
    }

    #[test]
    fn rpa_identity_matches_bluetooth_core_sample() {
        // Bluetooth Core ah() sample: IRK ec0234...7d9b, prand 708194,
        // hash 0dfbaa. Our stored IRK uses the protocol's little endian order.
        let key: [u8; 16] = bytes("9b7d390aa610103405adc857a33402ec")
            .try_into()
            .unwrap();
        assert!(verify_rpa("70:81:94:0D:FB:AA", &key));
        assert!(!verify_rpa("70:81:94:0D:FB:AB", &key));
        for malformed in [
            "",
            "70:81:94:0D:FB",
            "70:81:94:0D:FB:100",
            "70:81:94:0D:FB:GG",
        ] {
            assert!(!verify_rpa(malformed, &key));
        }
    }
}
