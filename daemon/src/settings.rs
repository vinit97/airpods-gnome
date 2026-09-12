// SPDX-License-Identifier: GPL-3.0-or-later
//! Private, atomic JSON settings.

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
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::default(),
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
}

fn invalid_data(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_settings_round_trip_with_private_permissions() {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let mut settings = Settings::load(directory.path()).unwrap();
        assert!(settings == Settings::default());
        assert!(!directory.path().join(FILE_NAME).exists());
        settings.ear_detection_behavior = 1;
        settings.adaptive_noise_level = 75;
        settings.conversational_awareness = true;
        settings.device_name = "Test AirPods".into();
        settings.magic_acc_irk = (0..16).collect();
        settings.magic_acc_enc_key = b"0123456789abcdef".to_vec();
        settings.save(directory.path()).unwrap();
        assert!(Settings::load(directory.path()).unwrap() == settings);
        assert_eq!(
            fs::metadata(directory.path().join(FILE_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(directory.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn json_preserves_unknown_fields() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(FILE_NAME),
            r#"{"ear_detection_behavior":2,"future_setting":{"value":42}}"#,
        )
        .unwrap();
        let mut settings = Settings::load(directory.path()).unwrap();
        assert_eq!(settings.ear_detection_behavior, 2);
        settings.adaptive_noise_level = 20;
        settings.save(directory.path()).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.path().join(FILE_NAME)).unwrap()).unwrap();
        assert_eq!(saved["future_setting"]["value"], 42);
    }

    #[test]
    fn corrupt_or_out_of_range_settings_are_not_silently_reset() {
        let directory = tempfile::tempdir().unwrap();
        for contents in [
            "{",
            r#"{"ear_detection_behavior":9}"#,
            r#"{"magic_acc_irk":[1]}"#,
            r#"{"adaptive_noise_level":101}"#,
        ] {
            fs::write(directory.path().join(FILE_NAME), contents).unwrap();
            assert!(Settings::load(directory.path()).is_err());
            assert_eq!(
                fs::read_to_string(directory.path().join(FILE_NAME)).unwrap(),
                contents
            );
        }
    }
}
