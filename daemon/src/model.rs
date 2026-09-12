// SPDX-License-Identifier: GPL-3.0-or-later
// Schema 1 compatibility with the LibrePods/Omapods GNOME status interface.

use crate::protocol::{Advertisement, Component, EarState, Event};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Battery {
    pub available: bool,
    pub level: u8,
    pub charging: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_ear: Option<bool>,
}

impl Battery {
    fn pod() -> Self {
        Self {
            in_ear: Some(false),
            ..Self::default()
        }
    }

    fn update(&mut self, level: u8, charging: bool) {
        if level <= 100 {
            self.available = true;
            self.level = level;
            self.charging = charging;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    pub schema_version: u8,
    pub connected: bool,
    pub device_name: String,
    pub noise_mode: i32,
    pub left: Battery,
    pub right: Battery,
    pub case: Battery,
    pub headset: Battery,
    pub reconnect_attempts_total: u64,
    pub reconnect_failures_total: u64,
    pub noise_control_changes_total: u64,
    pub forget_calls_total: u64,
    pub ear_detection_changes_total: u64,
    pub ca_changes_total: u64,
    pub disconnect_calls_total: u64,
    pub connect_calls_total: u64,
    pub disconnect_failures_total: u64,
    pub connect_failures_total: u64,
    pub adaptive_level_changes_total: u64,
    pub one_bud_anc_changes_total: u64,
    pub reopen_calls_total: u64,
    pub conversational_awareness: bool,
    pub adaptive_noise_level: u8,
    pub one_bud_anc_mode: bool,
    pub model_name: String,
    pub model_int: u8,
    pub is_pro_series: bool,
    pub is_headset: bool,
    pub supports_noise_off: bool,
    pub supports_noise_control: bool,
    pub supports_adaptive: bool,
    pub supports_conversational_awareness: bool,
    pub supports_one_bud_anc: bool,
    pub model_number: String,
    pub ear_detection_behavior: u8,
    pub lid_state: u8,
    #[serde(skip)]
    pub primary: Component,
    #[serde(skip)]
    pub primary_ear: EarState,
    #[serde(skip)]
    pub secondary_ear: EarState,
    #[serde(skip)]
    case_is_exact: bool,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            schema_version: 1,
            connected: false,
            device_name: String::new(),
            noise_mode: -1,
            left: Battery::pod(),
            right: Battery::pod(),
            case: Battery::default(),
            headset: Battery::default(),
            reconnect_attempts_total: 0,
            reconnect_failures_total: 0,
            noise_control_changes_total: 0,
            forget_calls_total: 0,
            ear_detection_changes_total: 0,
            ca_changes_total: 0,
            disconnect_calls_total: 0,
            connect_calls_total: 0,
            disconnect_failures_total: 0,
            connect_failures_total: 0,
            adaptive_level_changes_total: 0,
            one_bud_anc_changes_total: 0,
            reopen_calls_total: 0,
            conversational_awareness: false,
            adaptive_noise_level: 50,
            one_bud_anc_mode: false,
            model_name: String::new(),
            model_int: 0,
            is_pro_series: false,
            is_headset: false,
            supports_noise_off: true,
            supports_noise_control: true,
            supports_adaptive: false,
            supports_conversational_awareness: false,
            supports_one_bud_anc: false,
            model_number: String::new(),
            ear_detection_behavior: 1,
            lid_state: 2,
            primary: Component::Left,
            primary_ear: EarState::Disconnected,
            secondary_ear: EarState::Disconnected,
            case_is_exact: false,
        }
    }
}

impl Status {
    pub fn set_model_number(&mut self, number: &str) {
        self.model_number = number.to_owned();
        self.set_model_id(model_from_number(number));
    }

    pub fn set_model_id(&mut self, id: u8) {
        self.model_int = if id <= 12 { id } else { 0 };
        self.model_name = model_name(self.model_int).to_owned();
        self.is_pro_series = matches!(self.model_int, 4 | 5 | 6 | 11);
        self.is_headset = matches!(self.model_int, 7 | 8 | 12);
        self.supports_noise_off = self.model_int != 11;
        self.supports_noise_control = !matches!(self.model_int, 1 | 2 | 3 | 9);
        self.supports_adaptive = matches!(self.model_int, 5 | 6 | 10 | 11 | 12);
        self.supports_conversational_awareness = self.supports_adaptive;
        self.supports_one_bud_anc = matches!(self.model_int, 4 | 5 | 6 | 10 | 11);
    }

    pub fn ears_in(&self) -> (bool, bool) {
        if self.is_headset {
            let worn = self.primary_ear == EarState::InEar;
            return (worn, worn);
        }
        (
            self.left.in_ear == Some(true),
            self.right.in_ear == Some(true),
        )
    }

    /// Forget measurements when selecting a different device. A temporary
    /// control reconnect should keep the last measurements instead.
    pub fn reset_measurements(&mut self) {
        self.left = Battery::pod();
        self.right = Battery::pod();
        self.case = Battery::default();
        self.headset = Battery::default();
        self.primary = Component::Left;
        self.primary_ear = EarState::Disconnected;
        self.secondary_ear = EarState::Disconnected;
        self.lid_state = 2;
        self.case_is_exact = false;
    }

    fn update_ears(&mut self) {
        let primary = self.primary_ear == EarState::InEar;
        let secondary = self.secondary_ear == EarState::InEar;
        self.left.in_ear = Some(if self.primary == Component::Left {
            primary
        } else {
            secondary
        });
        self.right.in_ear = Some(if self.primary == Component::Right {
            primary
        } else {
            secondary
        });
    }

    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::Battery(readings) => {
                if let Some(reading) = readings.iter().find(|r| r.component != Component::Case) {
                    self.primary = reading.component;
                }
                for reading in readings {
                    // Disconnected means no new measurement, not a measured 0%.
                    if !reading.available {
                        continue;
                    }
                    let target = match reading.component {
                        Component::Headset => &mut self.headset,
                        Component::Left => &mut self.left,
                        Component::Right => &mut self.right,
                        Component::Case => &mut self.case,
                    };
                    target.update(reading.level, reading.charging);
                    if reading.component == Component::Case {
                        self.case_is_exact = true;
                    }
                }
                self.update_ears();
            }
            Event::Ear { primary, secondary } => {
                self.primary_ear = *primary;
                self.secondary_ear = *secondary;
                self.update_ears();
            }
            Event::NoiseMode(mode) => self.noise_mode = *mode,
            Event::AdaptiveLevel(level) => self.adaptive_noise_level = *level,
            Event::Conversation(enabled) => self.conversational_awareness = *enabled,
            Event::OneBudAnc(enabled) => self.one_bud_anc_mode = *enabled,
            Event::Metadata { name, model_number } => {
                self.device_name.clone_from(name);
                self.set_model_number(model_number);
            }
            Event::Disconnected => self.connected = false,
            _ => (),
        }
    }

    /// Call only after authenticating the advertisement's address with its IRK.
    pub fn apply_ble(&mut self, adv: &Advertisement, decrypted: &[u8; 16]) {
        let model = model_from_ble(adv.model_id);
        if model != 0 {
            self.set_model_id(model);
        }
        if self.is_headset {
            self.primary = Component::Headset;
            self.headset
                .update(decrypted[1] & 0x7f, decrypted[1] & 0x80 != 0);
        } else {
            self.primary = if adv.primary_left {
                Component::Left
            } else {
                Component::Right
            };
            let (left, right) = if adv.primary_left {
                (decrypted[1], decrypted[2])
            } else {
                (decrypted[2], decrypted[1])
            };
            self.left.update(left & 0x7f, left & 0x80 != 0);
            self.right.update(right & 0x7f, right & 0x80 != 0);
            let level = decrypted[3] & 0x7f;
            // A 0 case byte when neither pod is docked means unavailable.
            if (level != 0 || adv.pod_in_case) && level <= 100 {
                self.case.update(level, decrypted[3] & 0x80 != 0);
                self.case_is_exact = true;
            } else if !self.case_is_exact {
                // The public case nibble is useful before an exact reading is
                // available, but must not round a known exact level to tens.
                if let Some(level) = adv.case_level {
                    self.case.update(level, adv.case_charging);
                }
            }
        }
        self.primary_ear =
            if (adv.primary_left && adv.left_in_ear) || (!adv.primary_left && adv.right_in_ear) {
                EarState::InEar
            } else {
                EarState::Out
            };
        self.secondary_ear =
            if (adv.primary_left && adv.right_in_ear) || (!adv.primary_left && adv.left_in_ear) {
                EarState::InEar
            } else {
                EarState::Out
            };
        self.update_ears();
        self.lid_state = adv.lid_state;
    }
}

pub fn model_from_number(number: &str) -> u8 {
    match number {
        "A1523" | "A1722" => 1,
        "A2032" | "A2031" => 2,
        "A2565" | "A2564" => 3,
        "A2084" | "A2083" => 4,
        "A2931" | "A2699" | "A2698" => 5,
        "A3047" | "A3048" | "A3049" => 6,
        "A2096" => 7,
        "A3184" => 8,
        "A3053" | "A3050" | "A3054" => 9,
        "A3056" | "A3055" | "A3057" => 10,
        "A3063" | "A3064" | "A3065" => 11,
        "A3454" => 12,
        _ => 0,
    }
}

pub fn model_from_ble(id: u16) -> u8 {
    match id {
        0x0220 => 1,
        0x0f20 => 2,
        0x1320 => 3,
        0x0e20 => 4,
        0x1420 => 5,
        0x2420 => 6,
        0x0a20 => 7,
        0x1f20 => 8,
        0x1920 => 9,
        0x1b20 => 10,
        0x2720 => 11,
        _ => 0,
    }
}

pub fn model_name(id: u8) -> &'static str {
    match id {
        1 => "AirPods",
        2 => "AirPods (2nd generation)",
        3 => "AirPods (3rd generation)",
        4 => "AirPods Pro",
        5 => "AirPods Pro 2",
        6 => "AirPods Pro 2 (USB-C)",
        7 => "AirPods Max",
        8 => "AirPods Max (USB-C)",
        9 | 10 => "AirPods 4",
        11 => "AirPods Pro 3",
        12 => "AirPods Max 2",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ComponentReading;

    fn adv(primary_left: bool, pod_in_case: bool, case_level: Option<u8>) -> Advertisement {
        Advertisement {
            model_id: 0x2720,
            primary_left,
            pod_in_case,
            left_in_ear: true,
            right_in_ear: false,
            case_level,
            case_charging: true,
            lid_state: 0,
            encrypted_payload: [0; 16],
        }
    }

    #[test]
    fn independent_batteries_and_primary_ear_mapping() {
        let mut s = Status::default();
        s.apply(&Event::Ear {
            primary: EarState::InEar,
            secondary: EarState::Out,
        });
        s.apply(&Event::Battery(vec![
            ComponentReading {
                component: Component::Right,
                level: 87,
                charging: false,
                available: true,
            },
            ComponentReading {
                component: Component::Left,
                level: 66,
                charging: false,
                available: true,
            },
        ]));
        assert_eq!((s.left.level, s.right.level), (66, 87));
        assert_eq!(s.ears_in(), (false, true));
        s.apply(&Event::Battery(vec![ComponentReading {
            component: Component::Left,
            level: 255,
            charging: false,
            available: false,
        }]));
        assert_eq!(s.left.level, 66);
    }

    #[test]
    fn encrypted_batteries_follow_primary_flip_without_rounding() {
        let mut s = Status::default();
        let mut data = [0; 16];
        data[1] = 71;
        data[2] = 83;
        data[3] = 0x80 | 46;
        s.apply_ble(&adv(true, false, Some(50)), &data);
        assert_eq!((s.left.level, s.right.level, s.case.level), (71, 83, 46));
        assert!(s.case.charging);
        s.apply_ble(&adv(false, false, None), &data);
        assert_eq!((s.left.level, s.right.level), (83, 71));
        assert_eq!(s.ears_in(), (true, false));
    }

    #[test]
    fn case_falls_back_to_broadcast_without_overwriting_exact_data() {
        let mut s = Status::default();
        let mut data = [0x7f; 16];
        data[3] = 0;
        s.apply_ble(&adv(true, false, Some(60)), &data);
        assert!(s.case.available);
        assert_eq!(s.case.level, 60);
        assert!(!s.left.available);
        s.apply_ble(&adv(true, false, Some(70)), &data);
        assert_eq!(s.case.level, 70);
        data[3] = 63;
        s.apply_ble(&adv(true, false, Some(60)), &data);
        assert_eq!(s.case.level, 63);
        data[3] = 0x7f;
        s.apply_ble(&adv(true, false, Some(60)), &data);
        assert_eq!(s.case.level, 63);
        data[3] = 0;
        s.apply_ble(&adv(true, true, Some(0)), &data);
        assert_eq!(s.case.level, 0); // Docked 0 is a measured empty case.
    }

    #[test]
    fn unknown_and_invalid_decrypted_levels_preserve_last_known_values() {
        let mut s = Status::default();
        let mut data = [0; 16];
        data[1] = 41;
        data[2] = 63;
        data[3] = 54;
        s.apply_ble(&adv(true, false, None), &data);
        data[1] = 0xff;
        data[2] = 101;
        data[3] = 0;
        s.apply_ble(&adv(true, false, None), &data);
        assert_eq!((s.left.level, s.right.level, s.case.level), (41, 63, 54));
    }

    #[test]
    fn headset_never_gets_a_case_or_duplicate_pod_batteries() {
        let mut s = Status::default();
        let mut advertisement = adv(false, false, Some(0));
        advertisement.model_id = 0x0a20;
        let mut data = [0; 16];
        data[1] = 78;
        data[2] = 23;
        s.apply_ble(&advertisement, &data);
        assert_eq!(s.headset.level, 78);
        assert!(!s.left.available && !s.right.available && !s.case.available);
    }

    #[test]
    fn capability_matrix_and_unknown_ble_preserve_metadata_identity() {
        let mut s = Status::default();
        for (number, noise, adaptive, headset, off) in [
            ("A3064", true, true, false, false),
            ("A2084", true, false, false, true),
            ("A3053", false, false, false, true),
            ("A3056", true, true, false, true),
            ("A3454", true, true, true, true),
            ("A2096", true, false, true, true),
        ] {
            s.set_model_number(number);
            assert_eq!(
                (
                    s.supports_noise_control,
                    s.supports_adaptive,
                    s.is_headset,
                    s.supports_noise_off
                ),
                (noise, adaptive, headset, off)
            );
        }
        s.set_model_number("A3064");
        let mut unknown = adv(true, false, None);
        unknown.model_id = 0xffff;
        s.apply_ble(&unknown, &[0x7f; 16]);
        assert_eq!(s.model_int, 11);
    }

    #[test]
    fn status_json_keeps_schema_one_fields_and_hides_internal_tracking() {
        let s = Status::default();
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["left"]["in_ear"], false);
        assert!(json["case"].get("in_ear").is_none());
        assert!(json["headset"].get("in_ear").is_none());
        assert!(json.get("primary").is_none());
        assert_eq!(serde_json::from_value::<Status>(json).unwrap(), s);
    }
}
