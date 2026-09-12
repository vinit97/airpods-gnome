// SPDX-License-Identifier: GPL-3.0-or-later
//! Private, atomic settings with a read-only migration from the Qt backend.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

const FILE_NAME: &str = "rust-settings.json";
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Preserve the published values: 0 = I, 1 = II, 2 = Off.
    pub ear_detection_behavior: u8,
    pub conversational_awareness: bool,
    pub adaptive_noise_level: u8,
    pub one_bud_anc: bool,
    pub device_name: String,
    pub model_number: String,
    pub bluetooth_address: String,
    pub model: u8,
    pub magic_acc_irk: Vec<u8>,
    pub magic_acc_enc_key: Vec<u8>,
    /// Keep future settings when an older daemon rewrites a known preference.
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ear_detection_behavior: 0,
            conversational_awareness: false,
            adaptive_noise_level: 50,
            one_bud_anc: false,
            device_name: String::new(),
            model_number: String::new(),
            bluetooth_address: String::new(),
            model: 0,
            magic_acc_irk: Vec::new(),
            magic_acc_enc_key: Vec::new(),
            extra: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// `directory` is the existing $XDG_CONFIG_HOME/AirPodsTrayApp directory.
    /// Malformed JSON is an error, never a reason to replace saved preferences.
    pub fn load(directory: &Path) -> io::Result<Self> {
        let settings = match fs::read(directory.join(FILE_NAME)) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(invalid_data)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::read_to_string(directory.join("AirPodsTrayApp.conf")) {
                    Ok(contents) => Self::from_legacy(&contents)?,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Self::default(),
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };
        settings.validate()?;
        Ok(settings)
    }

    pub fn save(&self, directory: &Path) -> io::Result<()> {
        self.validate()?;
        fs::create_dir_all(directory)?;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        let temporary = directory.join(format!(
            ".rust-settings-{}-{}.tmp",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            serde_json::to_writer_pretty(&mut file, self).map_err(invalid_data)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, directory.join(FILE_NAME))?;
            fs::File::open(directory)?.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    fn validate(&self) -> io::Result<()> {
        if self.ear_detection_behavior > 2 || self.adaptive_noise_level > 100 {
            return Err(invalid_data("saved AirPods preference is out of range"));
        }
        for key in [&self.magic_acc_irk, &self.magic_acc_enc_key] {
            if !key.is_empty() && key.len() != 16 {
                return Err(invalid_data("saved AirPods pairing key has invalid length"));
            }
        }
        Ok(())
    }

    fn from_legacy(contents: &str) -> io::Result<Self> {
        let mut settings = Self::default();
        let mut group = "";
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                group = section;
                continue;
            }
            let Some((name, value)) = line.split_once('=') else {
                continue;
            };
            let key = format!("{}/{}", group, name.trim().replace('\\', "/"));
            let value = value.trim();
            match key.as_str() {
                "earDetection/setting" => {
                    settings.ear_detection_behavior = value.parse().map_err(invalid_data)?;
                }
                "DeviceInfo/conversationalAwareness" => {
                    settings.conversational_awareness = legacy_bool(value)?;
                }
                "DeviceInfo/adaptiveNoiseLevel" => {
                    settings.adaptive_noise_level = value.parse().map_err(invalid_data)?;
                }
                "DeviceInfo/oneBudANCMode" => settings.one_bud_anc = legacy_bool(value)?,
                "DeviceInfo/deviceName" => settings.device_name = legacy_string(value)?,
                "DeviceInfo/modelNumber" => settings.model_number = legacy_string(value)?,
                "DeviceInfo/bluetoothAddress" => settings.bluetooth_address = legacy_string(value)?,
                "DeviceInfo/model" => settings.model = value.parse().map_err(invalid_data)?,
                "DeviceInfo/magicAccIRK" => settings.magic_acc_irk = legacy_byte_array(value)?,
                "DeviceInfo/magicAccEncKey" => {
                    settings.magic_acc_enc_key = legacy_byte_array(value)?;
                }
                // Unsupported legacy controls stay in the original, untouched file.
                _ => {}
            }
        }
        settings.validate()?;
        Ok(settings)
    }
}

fn invalid_data(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn legacy_bool(value: &str) -> io::Result<bool> {
    match value {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(invalid_data("invalid legacy boolean preference")),
    }
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(value)
}

fn legacy_string(value: &str) -> io::Result<String> {
    String::from_utf8(unescape(unquote(value))?).map_err(invalid_data)
}

fn legacy_byte_array(value: &str) -> io::Result<Vec<u8>> {
    let value = unquote(value);
    let contents = value
        .strip_prefix("@ByteArray(")
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| invalid_data("invalid legacy pairing key encoding"))?;
    unescape(contents)
}

fn unescape(value: &str) -> io::Result<Vec<u8>> {
    let mut output = Vec::with_capacity(value.len());
    let mut bytes = value.bytes().peekable();
    while let Some(byte) = bytes.next() {
        if byte != b'\\' {
            output.push(byte);
            continue;
        }
        let escaped = bytes
            .next()
            .ok_or_else(|| invalid_data("unfinished legacy escape"))?;
        output.push(match escaped {
            b'0' => 0,
            b'a' => 7,
            b'b' => 8,
            b'f' => 12,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'v' => 11,
            b'\\' | b'"' | b'\'' => escaped,
            b'x' => {
                let mut number = 0_u8;
                let mut digits = 0;
                while digits < 2 {
                    let Some(digit) = bytes.peek().and_then(|b| (*b as char).to_digit(16)) else {
                        break;
                    };
                    bytes.next();
                    number = number * 16 + digit as u8;
                    digits += 1;
                }
                if digits == 0 {
                    return Err(invalid_data("invalid legacy hexadecimal escape"));
                }
                number
            }
            _ => return Err(invalid_data("unknown legacy escape")),
        });
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(std::path::PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "airpods-settings-test-{}-{}",
                std::process::id(),
                TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn migrates_preferences_and_binary_keys_without_touching_legacy() {
        let directory = TestDirectory::new();
        let legacy = concat!(
            "[earDetection]\nsetting=1\n[DeviceInfo]\n",
            "deviceName=\"AirPods, Pro\"\nmodelNumber=A3048\nmodel=6\n",
            "conversationalAwareness=true\noneBudANCMode=false\nadaptiveNoiseLevel=75\n",
            "magicAccIRK=@ByteArray(\\0\\x1\\x2\\x3\\x4\\x5\\x6\\x7\\b\\t\\n\\v\\f\\r\\xe\\xf)\n",
            "magicAccEncKey=@ByteArray(0123456789abcdef)\n",
            "[crossdevice]\nenabled=true\n"
        );
        fs::write(directory.0.join("AirPodsTrayApp.conf"), legacy).unwrap();
        let settings = Settings::load(&directory.0).unwrap();
        assert_eq!(settings.ear_detection_behavior, 1);
        assert_eq!(settings.adaptive_noise_level, 75);
        assert!(settings.conversational_awareness);
        assert_eq!(settings.device_name, "AirPods, Pro");
        assert_eq!(settings.magic_acc_irk, (0..16).collect::<Vec<u8>>());
        settings.save(&directory.0).unwrap();
        assert_eq!(
            fs::read_to_string(directory.0.join("AirPodsTrayApp.conf")).unwrap(),
            legacy
        );
        assert!(Settings::load(&directory.0).unwrap() == settings);
        assert_eq!(
            fs::metadata(directory.0.join(FILE_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&directory.0).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn json_precedes_legacy_and_preserves_unknown_fields() {
        let directory = TestDirectory::new();
        fs::write(
            directory.0.join(FILE_NAME),
            r#"{"ear_detection_behavior":2,"future_setting":{"value":42}}"#,
        )
        .unwrap();
        fs::write(
            directory.0.join("AirPodsTrayApp.conf"),
            "[earDetection]\nsetting=0",
        )
        .unwrap();
        let mut settings = Settings::load(&directory.0).unwrap();
        assert_eq!(settings.ear_detection_behavior, 2);
        settings.adaptive_noise_level = 20;
        settings.save(&directory.0).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.0.join(FILE_NAME)).unwrap()).unwrap();
        assert_eq!(saved["future_setting"]["value"], 42);
    }

    #[test]
    fn corrupt_or_out_of_range_settings_are_not_silently_reset() {
        let directory = TestDirectory::new();
        for contents in [
            "{",
            r#"{"ear_detection_behavior":9}"#,
            r#"{"magic_acc_irk":[1]}"#,
            r#"{"adaptive_noise_level":101}"#,
        ] {
            fs::write(directory.0.join(FILE_NAME), contents).unwrap();
            assert!(Settings::load(&directory.0).is_err());
            assert_eq!(
                fs::read_to_string(directory.0.join(FILE_NAME)).unwrap(),
                contents
            );
        }
    }

    #[test]
    fn malformed_legacy_keys_are_not_discarded() {
        assert!(Settings::from_legacy("[DeviceInfo]\nmagicAccIRK=@ByteArray(\\xgg)").is_err());
        assert!(Settings::from_legacy("[DeviceInfo]\nmagicAccIRK=@ByteArray(short)").is_err());
    }
}
