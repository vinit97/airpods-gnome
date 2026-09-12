// SPDX-License-Identifier: GPL-3.0-or-later
//! Headless MPRIS playback and WirePlumber audio control.
//!
//! Only transitions launch audio commands; the Bluetooth loop can keep this
//! controller in a separate task. No shell is used and each call has a deadline.

use anyhow::{Context, Result, anyhow, bail};
use futures_util::future::join_all;
use serde_json::Value;
use std::collections::HashSet;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::time::timeout;

const BOTH_OUT_SETTLE: Duration = Duration::from_millis(1200);
const AUDIO_TIMEOUT: Duration = Duration::from_secs(3);
const BUS_TIMEOUT: Duration = Duration::from_secs(1);
const PROFILE_RETRY: Duration = Duration::from_millis(1500);
const CAPTURE_RECHECK: Duration = Duration::from_secs(10);
const VOLUME_RETRY: Duration = Duration::from_millis(1500);
const MAX_VOLUME_RETRIES: u8 = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EarAction {
    Pause,
    Resume,
    Release,
}

#[derive(Default)]
struct EarPolicy {
    behavior: u8,
    ears: Option<(bool, bool)>,
    both_out: Option<Instant>,
}

impl EarPolicy {
    fn set_behavior(&mut self, behavior: u8) {
        self.behavior = behavior.min(2);
        self.both_out = None;
        self.ears = None;
    }

    fn update(&mut self, left: bool, right: bool, now: Instant) -> Option<EarAction> {
        if self.ears == Some((left, right)) {
            return None;
        }
        self.ears = Some((left, right));
        if self.behavior == 2 {
            return None;
        }
        if !left && !right {
            self.both_out.get_or_insert(now + BOTH_OUT_SETTLE);
            return None;
        }
        self.both_out = None;
        Some(if self.behavior == 0 && (!left || !right) {
            EarAction::Pause
        } else {
            EarAction::Resume
        })
    }

    fn settle(&mut self, now: Instant) -> Option<EarAction> {
        if self.both_out.is_some_and(|deadline| now >= deadline) {
            self.both_out = None;
            return Some(EarAction::Release);
        }
        None
    }
}

#[derive(Clone)]
struct Sink {
    id: u64,
    serial: u64,
    name: String,
}

#[derive(Clone)]
struct Duck {
    sink: Sink,
    original: f64,
    lowered: f64,
}

impl Duck {
    fn restore_volume(&self, current: f64) -> Option<f64> {
        // wpctl reports two decimals. A manual volume change takes precedence.
        ((current - self.lowered).abs() < 0.006).then_some(self.original)
    }
}

pub struct MediaController {
    bus: zbus::Connection,
    device: Option<String>,
    ears: EarPolicy,
    paused: HashSet<String>,
    duck: Option<Duck>,
    duck_restore_due: Option<Instant>,
    duck_restore_retries: u8,
    profile_due: Option<Instant>,
    profile_attempts: u8,
    pending_resume: bool,
}

impl MediaController {
    pub async fn new() -> Result<Self> {
        let bus = timeout(AUDIO_TIMEOUT, zbus::Connection::session())
            .await
            .context("session bus connection timed out")??;
        Ok(Self {
            bus,
            device: None,
            ears: EarPolicy::default(),
            paused: HashSet::new(),
            duck: None,
            duck_restore_due: None,
            duck_restore_retries: 0,
            profile_due: None,
            profile_attempts: 0,
            pending_resume: false,
        })
    }

    pub async fn set_device(&mut self, address: Option<&str>) -> Result<()> {
        let address = address.map(normalize_address);
        if self.device == address {
            return Ok(());
        }
        let restored = self.restore_duck().await;
        self.device = address;
        self.ears.ears = None;
        self.ears.both_out = None;
        self.paused.clear();
        self.pending_resume = false;
        self.profile_attempts = 0;
        self.profile_due = self.device.as_ref().map(|_| Instant::now() + PROFILE_RETRY);
        restored
    }

    pub fn set_ear_detection(&mut self, behavior: u8) {
        if self.ears.behavior != behavior {
            self.ears.set_behavior(behavior);
            // Disabling automatic playback must not start previously paused media.
            self.pending_resume = false;
            if behavior == 2 {
                self.paused.clear();
            }
        }
    }

    pub async fn update_ears(&mut self, left: bool, right: bool) -> Result<()> {
        if self.device.is_none() {
            return Ok(());
        }
        if let Some(action) = self.ears.update(left, right, Instant::now()) {
            self.apply_ears(action).await?;
        }
        Ok(())
    }

    pub async fn conversation_event(&mut self, event: u8) -> Result<()> {
        match event {
            1 => {
                // Renewed speech supersedes a restore queued by an earlier end.
                self.duck_restore_due = None;
                if self.duck.is_some() {
                    return Ok(());
                }
                let Some(address) = self.device.as_deref() else {
                    return Ok(());
                };
                let snapshot = Snapshot::read().await?;
                let Some(sink) = snapshot.default_sink(address) else {
                    return Ok(());
                };
                let original = volume(sink.id).await?;
                // Preserve the original backend's five-percent volume steps.
                let lowered = (original * 0.2 * 20.0).round() / 20.0;
                set_volume(sink.id, lowered).await?;
                self.duck = Some(Duck {
                    sink,
                    original,
                    lowered,
                });
            }
            9 => {}
            // Includes feature-disable event 8: restore instead of leaving it low.
            _ => self.restore_duck().await?,
        }
        Ok(())
    }

    pub async fn tick(&mut self) -> Result<()> {
        if let Some(action) = self.ears.settle(Instant::now()) {
            self.apply_ears(action).await?;
        }
        if self.profile_due.is_some_and(|due| Instant::now() >= due) {
            self.profile_due = None;
            self.recover_profile().await?;
        }
        if self
            .duck_restore_due
            .is_some_and(|due| Instant::now() >= due)
        {
            self.retry_restore_duck().await?;
        }
        Ok(())
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        [self.ears.both_out, self.profile_due, self.duck_restore_due]
            .into_iter()
            .flatten()
            .min()
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        self.profile_due = None;
        self.ears.both_out = None;
        self.pending_resume = false;
        self.paused.clear();
        self.restore_duck().await
    }

    async fn apply_ears(&mut self, action: EarAction) -> Result<()> {
        let Some(address) = self.device.clone() else {
            return Ok(());
        };
        if action == EarAction::Release {
            self.profile_due = None;
            self.pending_resume = false;
        } else {
            self.profile_due = Some(Instant::now());
            self.profile_attempts = 0;
            self.pending_resume = action == EarAction::Resume && !self.paused.is_empty();
        }
        let snapshot = Snapshot::read().await?;
        if action != EarAction::Resume && snapshot.default_sink(&address).is_some() {
            self.pause_players().await?;
        }
        if action == EarAction::Release && !snapshot.capturing(&address) {
            self.restore_duck().await?;
            if let Some((device_id, profile)) = snapshot.profile(&address, true)
                && !profile.active
            {
                set_profile(device_id, profile.index).await?;
            }
        }
        Ok(())
    }

    async fn recover_profile(&mut self) -> Result<()> {
        let Some(address) = self.device.clone() else {
            return Ok(());
        };
        let mut capture_deferred = false;
        let result = async {
            let snapshot = Snapshot::read().await?;
            // Calls retain their headset profile, including muted calls.
            if snapshot.capturing(&address) {
                self.profile_due = Some(Instant::now() + CAPTURE_RECHECK);
                capture_deferred = true;
                return Ok(());
            }
            let (device_id, profile) = snapshot
                .profile(&address, false)
                .ok_or_else(|| anyhow!("AirPods playback profile is not available yet"))?;
            if !profile.active {
                set_profile(device_id, profile.index).await?;
                // The new sink appears asynchronously; resume on the next check.
                if self.pending_resume {
                    self.profile_due = Some(Instant::now() + PROFILE_RETRY);
                }
            } else if self.pending_resume {
                if snapshot.default_sink(&address).is_some() {
                    self.resume_players().await?;
                    self.pending_resume = !self.paused.is_empty();
                }
                if self.pending_resume {
                    self.profile_due = Some(Instant::now() + PROFILE_RETRY);
                }
            }
            Ok(())
        }
        .await;
        if capture_deferred {
            return result;
        }
        self.profile_attempts = self.profile_attempts.saturating_add(1);
        if result.is_err() && self.profile_attempts < 6 {
            self.profile_due = Some(Instant::now() + PROFILE_RETRY);
        }
        if self.profile_attempts >= 6 {
            self.profile_due = None;
        }
        result
    }

    async fn restore_duck(&mut self) -> Result<()> {
        self.duck_restore_retries = 0;
        self.retry_restore_duck().await
    }

    async fn retry_restore_duck(&mut self) -> Result<()> {
        self.duck_restore_due = None;
        let Some(duck) = self.duck.clone() else {
            return Ok(());
        };
        let result = async {
            let snapshot = Snapshot::read().await?;
            // Node IDs can be reused. Name and serial ensure a different output
            // never receives the old AirPods volume, even after reconnecting.
            if let Some(sink) = snapshot.same_sink(&duck.sink)
                && let Some(original) = duck.restore_volume(volume(sink.id).await?)
            {
                set_volume(sink.id, original).await?;
            }
            Ok(())
        }
        .await;
        if result.is_ok() {
            self.duck = None;
        } else if self.duck_restore_retries < MAX_VOLUME_RETRIES {
            self.duck_restore_retries += 1;
            self.duck_restore_due = Some(Instant::now() + VOLUME_RETRY);
        }
        result
    }

    async fn pause_players(&mut self) -> Result<()> {
        let bus = zbus::Proxy::new(
            &self.bus,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .await?;
        let names: Vec<String> = timeout(BUS_TIMEOUT, bus.call("ListNames", &())).await??;
        let requests = names
            .into_iter()
            .filter(|name| name.starts_with("org.mpris.MediaPlayer2."))
            .map(|name| {
                let session = &self.bus;
                let bus = &bus;
                async move {
                    timeout(BUS_TIMEOUT, async {
                        // Unique owners prevent auto-playing a newly restarted application.
                        let owner: String = bus.call("GetNameOwner", &(name.as_str(),)).await?;
                        let player = zbus::Proxy::new(
                            session,
                            owner.as_str(),
                            "/org/mpris/MediaPlayer2",
                            "org.mpris.MediaPlayer2.Player",
                        )
                        .await?;
                        let status: String = player.get_property("PlaybackStatus").await?;
                        if status == "Playing" {
                            let _: () = player.call("Pause", &()).await?;
                            Ok::<_, zbus::Error>(Some(owner))
                        } else {
                            Ok(None)
                        }
                    })
                    .await
                }
            });
        for result in join_all(requests).await {
            if let Ok(Ok(Some(owner))) = result {
                self.paused.insert(owner);
            }
        }
        Ok(())
    }

    async fn resume_players(&mut self) -> Result<()> {
        let requests = self.paused.iter().cloned().map(|owner| {
            let session = &self.bus;
            async move {
                let result = timeout(BUS_TIMEOUT, async {
                    let player = zbus::Proxy::new(
                        session,
                        owner.as_str(),
                        "/org/mpris/MediaPlayer2",
                        "org.mpris.MediaPlayer2.Player",
                    )
                    .await?;
                    let status: String = player.get_property("PlaybackStatus").await?;
                    if status == "Paused" {
                        let _: () = player.call("Play", &()).await?;
                    }
                    Ok::<_, zbus::Error>(())
                })
                .await;
                (owner, result)
            }
        });
        for (owner, result) in join_all(requests).await {
            match result {
                Ok(Ok(())) => {
                    self.paused.remove(&owner);
                }
                Ok(Err(zbus::Error::MethodError(name, _, _)))
                    if matches!(
                        name.as_str(),
                        "org.freedesktop.DBus.Error.ServiceUnknown"
                            | "org.freedesktop.DBus.Error.NameHasNoOwner"
                    ) =>
                {
                    self.paused.remove(&owner);
                }
                _ => {}
            }
        }
        Ok(())
    }
}

async fn command(program: &str, arguments: &[&str]) -> Result<Vec<u8>> {
    let output = timeout(
        AUDIO_TIMEOUT,
        Command::new(program)
            .args(arguments)
            .env("LC_ALL", "C")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .with_context(|| format!("{program} timed out"))?
    .with_context(|| format!("could not run {program}"))?;
    if !output.status.success() {
        bail!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

async fn volume(id: u64) -> Result<f64> {
    let output = command("wpctl", &["get-volume", &id.to_string()]).await?;
    let text = std::str::from_utf8(&output)?;
    let value: f64 = text
        .strip_prefix("Volume:")
        .and_then(|rest| rest.split_whitespace().next())
        .ok_or_else(|| anyhow!("wpctl returned an invalid volume"))?
        .parse()?;
    if !value.is_finite() || !(0.0..=10.0).contains(&value) {
        bail!("wpctl returned an out-of-range volume");
    }
    Ok(value)
}

async fn set_volume(id: u64, volume: f64) -> Result<()> {
    command(
        "wpctl",
        &["set-volume", &id.to_string(), &format!("{volume:.4}")],
    )
    .await?;
    Ok(())
}

async fn set_profile(id: u64, index: u64) -> Result<()> {
    command(
        "wpctl",
        &["set-profile", &id.to_string(), &index.to_string()],
    )
    .await?;
    Ok(())
}

fn normalize_address(address: &str) -> String {
    address.replace([':', '-', '_'], "").to_ascii_uppercase()
}

fn integer(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

fn property<'a>(object: &'a Value, name: &str) -> &'a Value {
    &object["info"]["props"][name]
}

struct Profile {
    index: u64,
    active: bool,
}

struct Snapshot(Vec<Value>);

impl Snapshot {
    async fn read() -> Result<Self> {
        Ok(Self(serde_json::from_slice(
            &command("pw-dump", &[]).await?,
        )?))
    }

    fn device_id(&self, address: &str) -> Option<u64> {
        self.0.iter().find_map(|object| {
            if object["type"] != "PipeWire:Interface:Device" {
                return None;
            }
            let candidate = property(object, "api.bluez5.address").as_str()?;
            (normalize_address(candidate) == address).then(|| integer(&object["id"]))?
        })
    }

    fn belongs(&self, object: &Value, address: &str) -> bool {
        property(object, "api.bluez5.address")
            .as_str()
            .is_some_and(|candidate| normalize_address(candidate) == address)
            || self
                .device_id(address)
                .is_some_and(|id| integer(property(object, "device.id")) == Some(id))
    }

    fn sink(object: &Value) -> Option<Sink> {
        if property(object, "media.class") != "Audio/Sink" {
            return None;
        }
        Some(Sink {
            id: integer(&object["id"])?,
            serial: integer(property(object, "object.serial"))?,
            name: property(object, "node.name").as_str()?.to_owned(),
        })
    }

    fn default_sink(&self, address: &str) -> Option<Sink> {
        let name = self
            .0
            .iter()
            .filter_map(|object| object["metadata"].as_array())
            .flatten()
            .find(|entry| {
                entry["key"] == "default.audio.sink" && integer(&entry["subject"]) == Some(0)
            })?["value"]["name"]
            .as_str()?;
        self.0.iter().find_map(|object| {
            let sink = Self::sink(object)?;
            (sink.name == name && self.belongs(object, address)).then_some(sink)
        })
    }

    fn same_sink(&self, expected: &Sink) -> Option<Sink> {
        self.0.iter().find_map(|object| {
            let sink = Self::sink(object)?;
            (sink.serial == expected.serial && sink.name == expected.name).then_some(sink)
        })
    }

    fn capturing(&self, address: &str) -> bool {
        let sources: HashSet<u64> = self
            .0
            .iter()
            .filter(|object| {
                property(object, "media.class") == "Audio/Source" && self.belongs(object, address)
            })
            .filter_map(|object| integer(&object["id"]))
            .collect();
        self.0.iter().any(|link| {
            if link["type"] != "PipeWire:Interface:Link"
                || !integer(&link["info"]["output-node-id"]).is_some_and(|id| sources.contains(&id))
            {
                return false;
            }
            let input = integer(&link["info"]["input-node-id"]);
            self.0.iter().any(|node| {
                integer(&node["id"]) == input
                    && property(node, "media.class") == "Stream/Input/Audio"
                    && node["info"]["state"] == "running"
            })
        })
    }

    fn profile(&self, address: &str, off: bool) -> Option<(u64, Profile)> {
        let id = self.device_id(address)?;
        let device = self
            .0
            .iter()
            .find(|object| integer(&object["id"]) == Some(id))?;
        let parameters = &device["info"]["params"];
        let profiles = parameters["EnumProfile"].as_array()?;
        let selected = profiles
            .iter()
            .filter(|profile| {
                let name = profile["name"].as_str().unwrap_or_default();
                profile["available"] != "no"
                    && if off {
                        name == "off"
                    } else {
                        name.starts_with("a2dp-sink")
                    }
            })
            .max_by_key(|profile| {
                (
                    codec_rank(profile),
                    integer(&profile["priority"]).unwrap_or(0),
                )
            })?;
        let index = integer(&selected["index"])?;
        Some((
            id,
            Profile {
                index,
                active: parameters["Profile"]
                    .as_array()?
                    .iter()
                    .any(|profile| integer(&profile["index"]) == Some(index)),
            },
        ))
    }
}

fn codec_rank(profile: &Value) -> u16 {
    let name = profile["name"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace('_', "-");
    let description = profile["description"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.ends_with("sbc-xq") || description.contains("codec sbc-xq") {
        453
    } else if name.ends_with("sbc") || description.contains("codec sbc") {
        328
    } else if name.ends_with("aac") || description.contains("codec aac") {
        256
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::BufRead;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};

    #[test]
    fn both_out_debounce_cancels_transients_and_keeps_first_deadline() {
        let start = Instant::now();
        let mut ears = EarPolicy::default();
        assert_eq!(ears.update(true, true, start), Some(EarAction::Resume));
        assert_eq!(ears.update(false, false, start), None);
        assert_eq!(
            ears.update(false, false, start + Duration::from_millis(1100)),
            None
        );
        assert_eq!(
            ears.settle(start + BOTH_OUT_SETTLE),
            Some(EarAction::Release)
        );
        assert_eq!(ears.settle(start + BOTH_OUT_SETTLE), None);
        ears.update(true, true, start);
        ears.update(false, false, start);
        ears.update(true, true, start + Duration::from_millis(900));
        assert_eq!(ears.settle(start + BOTH_OUT_SETTLE), None);
    }

    #[test]
    fn ear_modes_match_legacy_values_and_disable_pending_work() {
        let now = Instant::now();
        let mut ears = EarPolicy::default();
        assert_eq!(ears.update(true, false, now), Some(EarAction::Pause));
        ears.set_behavior(1);
        assert_eq!(ears.update(true, false, now), Some(EarAction::Resume));
        ears.update(false, false, now);
        ears.set_behavior(2);
        assert_eq!(ears.settle(now + BOTH_OUT_SETTLE), None);
        assert_eq!(ears.update(true, false, now), None);
    }

    #[test]
    fn conversation_restore_respects_manual_volume_changes() {
        let duck = Duck {
            sink: Sink {
                id: 1,
                serial: 4,
                name: "airpods".into(),
            },
            original: 0.75,
            lowered: 0.15,
        };
        assert_eq!(duck.restore_volume(0.15), Some(0.75));
        assert_eq!(duck.restore_volume(0.35), None);
        assert_eq!(duck.restore_volume(0.16), None);
        assert_eq!(duck.restore_volume(0.0), None);
    }

    fn fixture() -> Snapshot {
        Snapshot(vec![
            json!({"id":1,"type":"PipeWire:Interface:Device","info":{"props":{"api.bluez5.address":"AA:BB:CC:DD:EE:FF"},"params":{
                "EnumProfile":[
                    {"index":0,"name":"off","available":"yes"},
                    {"index":10,"name":"a2dp-sink","description":"Playback (codec AAC)","available":"yes","priority":134},
                    {"index":11,"name":"a2dp-sink-sbc","available":"yes","priority":133},
                    {"index":12,"name":"a2dp-sink-sbc_xq","available":"yes","priority":132},
                    {"index":13,"name":"headset-head-unit-msbc","available":"yes","priority":999}],
                "Profile":[{"index":12}]}}}),
            json!({"id":2,"type":"PipeWire:Interface:Node","info":{"props":{"device.id":1,"media.class":"Audio/Sink","node.name":"pods","object.serial":22}}}),
            json!({"id":3,"type":"PipeWire:Interface:Node","info":{"props":{"device.id":1,"media.class":"Audio/Source","node.name":"pods-mic"}}}),
            json!({"id":4,"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"media.class":"Stream/Input/Audio"}}}),
            json!({"id":5,"type":"PipeWire:Interface:Metadata","metadata":[{"subject":0,"key":"default.audio.sink","value":{"name":"pods"}}]}),
        ])
    }

    #[test]
    fn profile_selection_preserves_codec_order_and_does_not_reapply_active_profile() {
        let snapshot = fixture();
        let (id, profile) = snapshot.profile("AABBCCDDEEFF", false).unwrap();
        assert_eq!(id, 1);
        assert_eq!(profile.index, 12);
        assert!(profile.active);
        assert_eq!(snapshot.profile("AABBCCDDEEFF", true).unwrap().1.index, 0);
        assert!(snapshot.profile("AABBCCDDEE00", false).is_none());
    }

    #[test]
    fn capture_requires_a_running_stream_attached_to_this_device_microphone() {
        let mut snapshot = fixture();
        assert!(!snapshot.capturing("AABBCCDDEEFF"));
        snapshot.0.push(
            json!({"type":"PipeWire:Interface:Link","info":{"output-node-id":3,"input-node-id":4}}),
        );
        assert!(snapshot.capturing("AABBCCDDEEFF"));
        assert!(!snapshot.capturing("001122334455"));
        snapshot.0[3]["info"]["state"] = json!("idle");
        assert!(!snapshot.capturing("AABBCCDDEEFF"));
        snapshot.0[3]["info"]["state"] = json!("running");
        snapshot.0.last_mut().unwrap()["info"]["output-node-id"] = json!(2);
        assert!(!snapshot.capturing("AABBCCDDEEFF"));
    }

    #[test]
    fn volume_restore_cannot_touch_a_reused_node_id() {
        let mut snapshot = fixture();
        let sink = snapshot.default_sink("AABBCCDDEEFF").unwrap();
        assert!(snapshot.same_sink(&sink).is_some());
        assert!(snapshot.default_sink("001122334455").is_none());
        snapshot.0[1]["info"]["props"]["object.serial"] = json!(23);
        assert!(snapshot.same_sink(&sink).is_none());
    }

    struct TestPlayer {
        state: Arc<Mutex<String>>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    #[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
    impl TestPlayer {
        #[zbus(property)]
        fn playback_status(&self) -> String {
            self.state.lock().unwrap().clone()
        }

        fn pause(&mut self) {
            *self.state.lock().unwrap() = "Paused".into();
            self.calls.lock().unwrap().push("Pause".into());
        }

        fn play(&mut self) {
            *self.state.lock().unwrap() = "Playing".into();
            self.calls.lock().unwrap().push("Play".into());
        }
    }

    async fn player(
        name: &str,
        state: &Arc<Mutex<String>>,
        calls: &Arc<Mutex<Vec<String>>>,
    ) -> zbus::Connection {
        zbus::connection::Builder::session()
            .unwrap()
            .name(name)
            .unwrap()
            .serve_at(
                "/org/mpris/MediaPlayer2",
                TestPlayer {
                    state: state.clone(),
                    calls: calls.clone(),
                },
            )
            .unwrap()
            .build()
            .await
            .unwrap()
    }

    /// Run with its own environment so fake commands cannot affect parallel tests
    /// or reach the user's sound server. The child PATH contains only our fixtures.
    #[test]
    fn media_io_end_to_end() {
        struct Bus(std::process::Child);
        impl Drop for Bus {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let bin = directory.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let fake = r#"#!/usr/bin/python3
import json, pathlib, sys, time
root = pathlib.Path(__file__).parent.parent
if (root / 'fail').exists():
    sys.stderr.write('fixture audio failure\n')
    sys.exit(7)
if (root / 'hang').exists():
    time.sleep(30)
if pathlib.Path(__file__).name == 'pw-dump':
    print((root / 'snapshot.json').read_text())
else:
    args = sys.argv[1:]
    if args[0] == 'get-volume':
        print('Volume: ' + (root / 'volume').read_text())
    elif args[0] == 'set-volume':
        (root / 'volume').write_text(str(float(args[2])))
    elif args[0] == 'set-profile':
        snapshot = json.loads((root / 'snapshot.json').read_text())
        for obj in snapshot:
            if obj.get('id') == int(args[1]):
                params = obj['info']['params']
                params['Profile'] = [next(p for p in params['EnumProfile'] if p['index'] == int(args[2]))]
        (root / 'snapshot.json').write_text(json.dumps(snapshot))
    else:
        sys.exit(8)
    with (root / 'calls').open('a') as out:
        out.write(json.dumps(args) + '\n')
"#;
        for program in ["wpctl", "pw-dump"] {
            let path = bin.join(program);
            std::fs::write(&path, fake).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        std::fs::write(
            directory.path().join("snapshot.json"),
            serde_json::to_vec(&fixture().0).unwrap(),
        )
        .unwrap();
        std::fs::write(directory.path().join("volume"), "0.75").unwrap();
        let mut bus = Bus(std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("dbus-daemon is needed for the private media integration test"));
        let mut address = String::new();
        std::io::BufReader::new(bus.0.stdout.as_mut().unwrap())
            .read_line(&mut address)
            .unwrap();
        assert!(
            !address.is_empty(),
            "private D-Bus daemon did not publish an address"
        );
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "media::tests::media_io_scenario",
                "--ignored",
                "--nocapture",
            ])
            .env("AIRPODS_MEDIA_IO_ROOT", directory.path())
            .env("DBUS_SESSION_BUS_ADDRESS", address.trim())
            .env("PATH", bin)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    #[ignore = "launched by media_io_end_to_end in an isolated child process"]
    async fn media_io_scenario() {
        let directory = std::path::PathBuf::from(
            std::env::var_os("AIRPODS_MEDIA_IO_ROOT").expect("requires isolated parent test"),
        );
        let state = Arc::new(Mutex::new("Playing".to_owned()));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let manually_paused = Arc::new(Mutex::new("Paused".to_owned()));
        let manual_calls = Arc::new(Mutex::new(Vec::new()));
        let service = player("org.mpris.MediaPlayer2.AirPodsTest", &state, &calls).await;
        let _manual = player(
            "org.mpris.MediaPlayer2.AirPodsManual",
            &manually_paused,
            &manual_calls,
        )
        .await;
        let mut controller = MediaController::new().await.unwrap();
        assert_eq!(controller.next_deadline(), None);
        controller
            .set_device(Some("AA:BB:CC:DD:EE:FF"))
            .await
            .unwrap();
        controller.update_ears(true, false).await.unwrap();
        assert_eq!(*state.lock().unwrap(), "Paused");
        assert_eq!(controller.paused.len(), 1);
        controller.update_ears(true, false).await.unwrap();
        assert_eq!(*calls.lock().unwrap(), ["Pause"]);
        controller.update_ears(true, true).await.unwrap();
        controller.tick().await.unwrap();
        assert_eq!(*state.lock().unwrap(), "Playing");
        assert_eq!(*calls.lock().unwrap(), ["Pause", "Play"]);
        assert!(
            manual_calls.lock().unwrap().is_empty(),
            "never resume a player the user paused"
        );

        controller.update_ears(true, false).await.unwrap();
        *state.lock().unwrap() = "Stopped".into();
        controller.update_ears(true, true).await.unwrap();
        controller.tick().await.unwrap();
        assert_eq!(
            *state.lock().unwrap(),
            "Stopped",
            "respect a manual stop while removed"
        );
        *state.lock().unwrap() = "Playing".into();
        controller.update_ears(true, false).await.unwrap();
        service.close().await.unwrap();
        let replacement_state = Arc::new(Mutex::new("Paused".to_owned()));
        let replacement_calls = Arc::new(Mutex::new(Vec::new()));
        let _replacement = player(
            "org.mpris.MediaPlayer2.AirPodsTest",
            &replacement_state,
            &replacement_calls,
        )
        .await;
        controller.update_ears(true, true).await.unwrap();
        controller.tick().await.unwrap();
        assert!(
            replacement_calls.lock().unwrap().is_empty(),
            "do not resume a restarted player with the old name"
        );

        controller.set_ear_detection(2);
        *replacement_state.lock().unwrap() = "Playing".into();
        controller.update_ears(false, false).await.unwrap();
        controller.ears.both_out = None;
        assert!(replacement_calls.lock().unwrap().is_empty());
        controller.set_ear_detection(1);
        controller.update_ears(true, false).await.unwrap();
        assert!(
            replacement_calls.lock().unwrap().is_empty(),
            "II keeps playing with one earbud"
        );
        controller.update_ears(false, false).await.unwrap();
        controller.tick().await.unwrap();
        assert!(
            replacement_calls.lock().unwrap().is_empty(),
            "both-out must settle before pausing"
        );
        controller.ears.both_out = Some(Instant::now());
        controller.tick().await.unwrap();
        assert_eq!(*replacement_state.lock().unwrap(), "Paused");
        let snapshot: Vec<Value> =
            serde_json::from_slice(&std::fs::read(directory.join("snapshot.json")).unwrap())
                .unwrap();
        assert_eq!(snapshot[0]["info"]["params"]["Profile"][0]["name"], "off");
        controller.update_ears(true, false).await.unwrap();
        controller.tick().await.unwrap();
        controller.profile_due = Some(Instant::now());
        controller.tick().await.unwrap();
        assert_eq!(*replacement_state.lock().unwrap(), "Playing");

        let read_volume = || {
            std::fs::read_to_string(directory.join("volume"))
                .unwrap()
                .parse::<f64>()
                .unwrap()
        };
        controller.conversation_event(1).await.unwrap();
        assert!((read_volume() - 0.15).abs() < 0.001);
        controller.conversation_event(1).await.unwrap();
        controller.conversation_event(0).await.unwrap();
        assert!((read_volume() - 0.75).abs() < 0.001);
        controller.conversation_event(1).await.unwrap();
        std::fs::write(directory.join("volume"), "0.42").unwrap();
        controller.conversation_event(1).await.unwrap();
        controller.conversation_event(0).await.unwrap();
        assert!(
            (read_volume() - 0.42).abs() < 0.001,
            "preserve a manual volume change during speech"
        );
        std::fs::write(directory.join("volume"), "0.75").unwrap();
        controller.conversation_event(1).await.unwrap();
        controller.conversation_event(8).await.unwrap();
        assert!(
            (read_volume() - 0.75).abs() < 0.001,
            "disabling awareness restores volume"
        );
        controller.conversation_event(9).await.unwrap();
        assert!((read_volume() - 0.75).abs() < 0.001);
        controller.conversation_event(1).await.unwrap();
        controller.shutdown().await.unwrap();
        assert!(
            (read_volume() - 0.75).abs() < 0.001,
            "shutdown restores volume"
        );
        assert_eq!(controller.next_deadline(), None);

        // A failed end event must recover without another Bluetooth event.
        controller.conversation_event(1).await.unwrap();
        std::fs::write(directory.join("fail"), "").unwrap();
        assert!(controller.conversation_event(0).await.is_err());
        assert!((read_volume() - 0.15).abs() < 0.001);
        assert!(controller.duck_restore_due.is_some());
        assert_eq!(controller.next_deadline(), controller.duck_restore_due);
        std::fs::remove_file(directory.join("fail")).unwrap();
        controller.duck_restore_due = Some(Instant::now());
        controller.tick().await.unwrap();
        assert!((read_volume() - 0.75).abs() < 0.001);
        assert_eq!(controller.next_deadline(), None);

        // A new start cancels a previous end's retry while speech is active.
        controller.conversation_event(1).await.unwrap();
        std::fs::write(directory.join("fail"), "").unwrap();
        assert!(controller.conversation_event(0).await.is_err());
        std::fs::remove_file(directory.join("fail")).unwrap();
        controller.duck_restore_due = Some(Instant::now());
        controller.conversation_event(1).await.unwrap();
        controller.tick().await.unwrap();
        assert!((read_volume() - 0.15).abs() < 0.001);
        assert_eq!(controller.next_deadline(), None);
        controller.conversation_event(0).await.unwrap();

        // Retry the same identity and manual-volume checks as an immediate end.
        controller.conversation_event(1).await.unwrap();
        std::fs::write(directory.join("fail"), "").unwrap();
        assert!(controller.conversation_event(0).await.is_err());
        std::fs::remove_file(directory.join("fail")).unwrap();
        std::fs::write(directory.join("volume"), "0.42").unwrap();
        controller.duck_restore_due = Some(Instant::now());
        controller.tick().await.unwrap();
        assert!((read_volume() - 0.42).abs() < 0.001);
        assert!(controller.duck.is_none());

        // Persistent command failures stop scheduling after the bounded retries.
        std::fs::write(directory.join("volume"), "0.75").unwrap();
        controller.conversation_event(1).await.unwrap();
        std::fs::write(directory.join("fail"), "").unwrap();
        assert!(controller.conversation_event(0).await.is_err());
        for _ in 0..MAX_VOLUME_RETRIES {
            assert!(controller.duck_restore_due.is_some());
            controller.duck_restore_due = Some(Instant::now());
            assert!(controller.tick().await.is_err());
        }
        assert_eq!(controller.next_deadline(), None);
        assert!(controller.duck.is_some());
        std::fs::remove_file(directory.join("fail")).unwrap();
        controller.conversation_event(0).await.unwrap();
        assert!((read_volume() - 0.75).abs() < 0.001);

        std::fs::rename(
            directory.join("bin/wpctl"),
            directory.join("bin/wpctl-disabled"),
        )
        .unwrap();
        assert!(
            controller
                .conversation_event(1)
                .await
                .unwrap_err()
                .to_string()
                .contains("could not run wpctl")
        );
        std::fs::rename(
            directory.join("bin/wpctl-disabled"),
            directory.join("bin/wpctl"),
        )
        .unwrap();
        std::fs::write(directory.join("fail"), "").unwrap();
        assert!(
            controller
                .conversation_event(1)
                .await
                .unwrap_err()
                .to_string()
                .contains("fixture audio failure")
        );
        std::fs::remove_file(directory.join("fail")).unwrap();
        std::fs::write(directory.join("hang"), "").unwrap();
        let started = Instant::now();
        assert!(
            controller
                .conversation_event(1)
                .await
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        assert!(started.elapsed() < Duration::from_secs(6));
        std::fs::remove_file(directory.join("hang")).unwrap();
        controller.conversation_event(1).await.unwrap();
        controller.shutdown().await.unwrap();
        assert!((read_volume() - 0.75).abs() < 0.001);
    }
}
