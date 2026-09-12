// SPDX-License-Identifier: GPL-3.0-or-later
use airpods_gnome_daemon::{
    bluetooth::{self, Bluetooth},
    ipc,
    media::MediaController,
    model::Status,
    protocol::{self, Event},
    settings::Settings,
};
use anyhow::{Context, Result};
use std::{fs, path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{mpsc, oneshot, watch},
    time::{Instant, timeout},
};

#[derive(Clone, Default, PartialEq)]
struct MediaState {
    device: Option<String>,
    behavior: u8,
    ears: Option<(bool, bool)>,
    conversation: Option<u8>,
}

struct Request {
    command: String,
    reply: oneshot::Sender<String>,
}

struct App {
    status: Status,
    settings: Settings,
    config: PathBuf,
    state: PathBuf,
    last_state: Vec<u8>,
    bluetooth: Bluetooth,
    media: watch::Sender<MediaState>,
    notifications_due: Option<Instant>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("AirPods backend: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    #[allow(unused_mut)]
    let mut test_socket = None;
    let mut args = std::env::args().skip(1);
    // The test transport consumes the following argument in fixture builds.
    #[cfg_attr(not(feature = "test-support"), allow(clippy::while_let_on_iterator))]
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--headless" => {}
            "--help" | "-h" => {
                println!("Usage: airpods-gnome [--headless]\nHeadless AirPods controls for GNOME.");
                return Ok(());
            }
            "--version" => {
                println!("airpods-gnome {} (Rust backend)", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            #[cfg(feature = "test-support")]
            "--test-transport" => {
                test_socket = Some(PathBuf::from(
                    args.next().context("Missing test transport path")?,
                ));
            }
            _ => anyhow::bail!("Unknown argument: {arg}"),
        }
    }
    let socket = ipc::socket_path()?;
    let _lock = ipc::daemon_lock(&socket)?;
    let state_dir = ipc::user_dir("XDG_STATE_HOME", ".local/state")?.join("librepods");
    let state = state_dir.join("status.json");
    let _cleanup = Cleanup {
        socket: socket.clone(),
        state: state.clone(),
    };
    // Holding the lock makes stale socket replacement safe even after SIGKILL.
    if socket.exists() {
        fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    ipc::private_dir(&state_dir)?;
    let config = ipc::user_dir("XDG_CONFIG_HOME", ".config")?.join("AirPodsTrayApp");
    let settings = Settings::load(&config)?;
    let (events_tx, mut events) = mpsc::channel(128);
    let test_mode = test_socket.is_some();
    let bluetooth = Bluetooth::start(events_tx, test_socket).await?;
    let (media, media_rx) = watch::channel(MediaState {
        behavior: settings.ear_detection_behavior,
        ..Default::default()
    });
    let media_task = if test_mode {
        None
    } else {
        Some(tokio::spawn(media_worker(media_rx)))
    };
    let mut status = Status::default();
    status.set_model_id(settings.model);
    if !settings.model_number.is_empty() {
        status.set_model_number(&settings.model_number);
    }
    status.device_name = settings.device_name.clone();
    status.ear_detection_behavior = settings.ear_detection_behavior;
    status.conversational_awareness = settings.conversational_awareness;
    status.adaptive_noise_level = settings.adaptive_noise_level;
    status.one_bud_anc_mode = settings.one_bud_anc;
    let mut app = App {
        status,
        settings,
        config,
        state,
        last_state: Vec::new(),
        bluetooth,
        media,
        notifications_due: None,
    };
    app.publish()?;
    let (requests_tx, mut requests) = mpsc::channel::<Request>(32);
    let clients = tokio::spawn(accept_clients(listener, requests_tx));
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    eprintln!("AirPods Rust backend ready");
    let result = loop {
        tokio::select! {
            _=term.recv()=>break Ok(()),
            _=tokio::signal::ctrl_c()=>break Ok(()),
            request=requests.recv()=>if let Some(request)=request {
                // Clients that gave up must not cause delayed surprise control changes.
                if request.reply.is_closed(){continue;}
                let response=match app.command(&request.command, &mut events).await {
                    Ok(response)=>response,
                    Err(error)=>{eprintln!("AirPods control: {error}");format!("error: {error}\n")},
                };
                if let Err(error)=app.publish(){break Err(error);}
                let _=request.reply.send(response);
            },
            event=events.recv()=>match event {
                Some(event)=>{
                    if let Err(error)=app.event(event).await {eprintln!("AirPods update: {error}");}
                    if let Err(error)=app.publish(){break Err(error);}
                },
                None=>break Err(anyhow::anyhow!("Bluetooth worker stopped")),
            },
            _=wait_until(app.notifications_due)=>{
                app.notifications_due=None;
                if !app.status.connected {
                    app.bluetooth.reconnect().await;
                } else if !app.status.left.available && !app.status.headset.available {
                    let _=app.bluetooth.send(protocol::NOTIFICATIONS).await;
                }
            }
        }
    };
    clients.abort();
    let _ = clients.await;
    app.status.connected = false;
    let _ = app.publish();
    app.bluetooth.shutdown().await;
    drop(app.media);
    if let Some(task) = media_task {
        let _ = timeout(Duration::from_secs(15), task).await;
    }
    result
}

impl App {
    fn publish(&mut self) -> Result<()> {
        let mut data = serde_json::to_vec(&self.status)?;
        data.push(b'\n');
        if data != self.last_state {
            ipc::atomic_write(&self.state, &data)?;
            self.last_state = data;
        }
        Ok(())
    }

    async fn event(&mut self, event: bluetooth::Event) -> Result<()> {
        match event {
            bluetooth::Event::Connected(device) => {
                if !self.settings.bluetooth_address.is_empty()
                    && !self
                        .settings
                        .bluetooth_address
                        .eq_ignore_ascii_case(&device.address)
                {
                    self.status.reset_measurements();
                    self.status.set_model_id(0);
                    self.status.model_number.clear();
                    self.settings.magic_acc_irk.clear();
                    self.settings.magic_acc_enc_key.clear();
                    self.settings.model = 0;
                    self.settings.model_number.clear();
                }
                self.settings.bluetooth_address = device.address.clone();
                self.status.connected = false;
                self.status.device_name = device.name;
                self.status.noise_mode = -1;
                self.media.send_modify(|m| {
                    m.device = Some(device.address);
                    m.ears = None;
                    m.conversation = None;
                });
                self.bluetooth.send(protocol::HANDSHAKE).await?;
                self.notifications_due = Some(Instant::now() + Duration::from_secs(3));
            }
            bluetooth::Event::Disconnected => {
                self.status.connected = false;
                self.notifications_due = None;
                self.media.send_modify(|m| {
                    m.device = None;
                    m.ears = None;
                    m.conversation = None;
                });
            }
            bluetooth::Event::Attempt => self.status.reconnect_attempts_total += 1,
            bluetooth::Event::Failed => self.status.reconnect_failures_total += 1,
            bluetooth::Event::Advertisement(address, data) => {
                if let (Ok(irk), Ok(key)) = (
                    self.settings.magic_acc_irk.as_slice().try_into(),
                    self.settings.magic_acc_enc_key.as_slice().try_into(),
                ) && protocol::verify_rpa(&address, &irk)
                    && let Some(ad) = protocol::parse_advertisement(&data)
                    && let Some(bytes) = protocol::decrypt_battery(&ad.encrypted_payload, &key)
                {
                    self.status.apply_ble(&ad, &bytes);
                }
            }
            bluetooth::Event::Packet(data) => {
                let Ok(Some(event)) = protocol::parse_packet(&data) else {
                    return Ok(());
                };
                self.status.apply(&event);
                match event {
                    Event::HandshakeAck => {
                        self.status.connected = true;
                        self.bluetooth.send(protocol::FEATURES).await?;
                    }
                    Event::FeaturesAck => {
                        self.bluetooth.send(protocol::NOTIFICATIONS).await?;
                        self.bluetooth.send(protocol::REQUEST_MAGIC_KEYS).await?;
                    }
                    Event::MagicKeys(keys) => {
                        self.settings.magic_acc_irk = keys.irk.to_vec();
                        self.settings.magic_acc_enc_key = keys.key.to_vec();
                        self.settings.save(&self.config)?;
                    }
                    Event::Metadata { .. } => {
                        self.settings.device_name = self.status.device_name.clone();
                        self.settings.model = self.status.model_int;
                        self.settings.model_number = self.status.model_number.clone();
                        self.settings.save(&self.config)?;
                        self.bluetooth.send(protocol::REQUEST_MAGIC_KEYS).await?;
                    }
                    Event::Disconnected => {
                        self.status.connected = false;
                        self.media.send_modify(|m| {
                            m.device = None;
                            m.ears = None;
                            m.conversation = None;
                        });
                    }
                    Event::Ear { .. } | Event::Battery(_) => {
                        let ears = self.status.ears_in();
                        self.media.send_modify(|m| m.ears = Some(ears));
                    }
                    Event::ConversationActivity(activity) => {
                        self.media.send_modify(|m| m.conversation = Some(activity));
                    }
                    Event::Conversation(false) => {
                        self.media.send_modify(|m| m.conversation = Some(8));
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    async fn command(
        &mut self,
        command: &str,
        events: &mut mpsc::Receiver<bluetooth::Event>,
    ) -> Result<String> {
        if command == "status" {
            return Ok(serde_json::to_string(&self.status)? + "\n");
        }
        let mut settings = self.settings.clone();
        let mut packet = None;
        let mut noise = None;
        match command {
            "ear:one" => settings.ear_detection_behavior = 0,
            "ear:both" => settings.ear_detection_behavior = 1,
            "ear:off" => settings.ear_detection_behavior = 2,
            "noise:off" | "noise:anc" | "noise:transparency" | "noise:adaptive" | "noise:cycle" => {
                anyhow::ensure!(self.status.connected, "AirPods are disconnected");
                anyhow::ensure!(
                    self.status.supports_noise_control,
                    "Listening modes are unavailable for this model"
                );
                let mode = match command {
                    "noise:off" => 0,
                    "noise:anc" => 1,
                    "noise:transparency" => 2,
                    "noise:adaptive" => 3,
                    _ => {
                        let available: Vec<i32> = (0..4)
                            .filter(|m| {
                                (*m != 0 || self.status.supports_noise_off)
                                    && (*m != 3 || self.status.supports_adaptive)
                            })
                            .collect();
                        let index = available
                            .iter()
                            .position(|m| *m == self.status.noise_mode)
                            .map_or(0, |i| (i + 1) % available.len());
                        available[index]
                    }
                };
                anyhow::ensure!(
                    mode != 0 || self.status.supports_noise_off,
                    "Off mode is unavailable for this model"
                );
                anyhow::ensure!(
                    mode != 3 || self.status.supports_adaptive,
                    "Adaptive mode is unavailable for this model"
                );
                if mode == self.status.noise_mode {
                    return Ok("ok\n".into());
                }
                packet = protocol::noise_packet(mode).map(|p| p.to_vec());
                noise = Some(mode);
            }
            "ca:on" | "ca:off" => {
                anyhow::ensure!(
                    self.status.supports_conversational_awareness,
                    "Conversation Awareness is unavailable for this model"
                );
                settings.conversational_awareness = command == "ca:on";
                packet =
                    Some(protocol::conversation_packet(settings.conversational_awareness).to_vec());
            }
            "onebud:on" | "onebud:off" => {
                anyhow::ensure!(
                    self.status.supports_one_bud_anc,
                    "One-bud ANC is unavailable for this model"
                );
                settings.one_bud_anc = command == "onebud:on";
                packet = Some(protocol::one_bud_packet(settings.one_bud_anc).to_vec());
            }
            "connect" | "disconnect" => {
                self.bluetooth
                    .device_command(
                        if command == "connect" {
                            "Connect"
                        } else {
                            "Disconnect"
                        },
                        Some(&self.settings.bluetooth_address),
                    )
                    .await?;
                return Ok("ok\n".into());
            }
            "reopen" => anyhow::bail!("this daemon runs headless and has no window"),
            _ if command.starts_with("adaptive:") => {
                let value = command.trim_start_matches("adaptive:");
                let level: u8 = value.parse().context("Adaptive level must be 0–100")?;
                anyhow::ensure!(
                    level <= 100 && level.to_string() == value,
                    "Adaptive level must be 0–100"
                );
                anyhow::ensure!(
                    self.status.supports_adaptive && self.status.noise_mode == 3,
                    "Adaptive listening mode is not active"
                );
                settings.adaptive_noise_level = level;
                packet = Some(
                    protocol::adaptive_packet(level)
                        .context("Invalid adaptive level")?
                        .to_vec(),
                );
            }
            _ => anyhow::bail!("Unknown command"),
        }
        if let Some(packet) = packet {
            self.bluetooth.send(&packet).await?;
            if let Some(mode) = noise {
                // A successful write does not mean the AirPods accepted the
                // mode. Keep publishing device reports while awaiting its ACK.
                let deadline = Instant::now() + Duration::from_millis(1800);
                loop {
                    let event = tokio::time::timeout_at(deadline, events.recv())
                        .await
                        .context("AirPods did not confirm the listening mode")?
                        .context("Bluetooth worker stopped")?;
                    let confirmed = matches!(&event, bluetooth::Event::Packet(bytes)
                        if protocol::parse_packet(bytes)==Ok(Some(Event::NoiseMode(mode))));
                    self.event(event).await?;
                    self.publish()?;
                    anyhow::ensure!(
                        self.status.connected,
                        "AirPods disconnected while changing listening mode"
                    );
                    if confirmed {
                        break;
                    }
                }
            }
        }
        if noise.is_none() && settings != self.settings {
            settings.save(&self.config)?;
            self.settings = settings;
        }
        if let Some(mode) = noise {
            self.status.noise_mode = mode;
            self.status.noise_control_changes_total += 1;
        }
        self.status.ear_detection_behavior = self.settings.ear_detection_behavior;
        if command.starts_with("ear:") {
            self.status.ear_detection_changes_total += 1;
            self.media
                .send_modify(|m| m.behavior = self.settings.ear_detection_behavior);
        }
        if command.starts_with("ca:") {
            self.status.conversational_awareness = self.settings.conversational_awareness;
            self.status.ca_changes_total += 1;
            if !self.status.conversational_awareness {
                self.media.send_modify(|m| m.conversation = Some(8));
            }
        }
        if command.starts_with("adaptive:") {
            self.status.adaptive_noise_level = self.settings.adaptive_noise_level;
            self.status.adaptive_level_changes_total += 1;
        }
        if command.starts_with("onebud:") {
            self.status.one_bud_anc_mode = self.settings.one_bud_anc;
            self.status.one_bud_anc_changes_total += 1;
        }
        Ok("ok\n".into())
    }
}

async fn accept_clients(listener: UnixListener, requests: mpsc::Sender<Request>) {
    let mut clients = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            result=listener.accept()=>match result {
                Ok((socket,_))=>{
                    if clients.len()>=32 {drop(socket);continue;}
                    let requests=requests.clone();
                    clients.spawn(async move {let _=timeout(Duration::from_secs(4),serve_client(socket,requests)).await;});
                },
                Err(_)=>break,
            },
            _=clients.join_next(),if !clients.is_empty()=>{}
        }
    }
}

async fn serve_client(mut socket: UnixStream, requests: mpsc::Sender<Request>) -> Result<()> {
    let mut bytes = Vec::new();
    loop {
        let byte = timeout(Duration::from_secs(1), socket.read_u8()).await;
        match byte {
            Ok(Ok(b'\n')) => break,
            Ok(Ok(byte)) if bytes.len() < 128 => bytes.push(byte),
            Ok(Err(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof && !bytes.is_empty() =>
            {
                break;
            }
            _ => {
                socket
                    .write_all(b"error: Invalid or incomplete command\n")
                    .await?;
                return Ok(());
            }
        }
    }
    let command = String::from_utf8(bytes).context("Invalid command encoding")?;
    let (reply, receive) = oneshot::channel();
    requests.send(Request { command, reply }).await?;
    let response = receive.await?;
    socket.write_all(response.as_bytes()).await?;
    socket.shutdown().await?;
    Ok(())
}

async fn wait_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

async fn media_worker(mut state: watch::Receiver<MediaState>) {
    let mut media = match MediaController::new().await {
        Ok(media) => media,
        Err(error) => {
            eprintln!("Media integration: {error}");
            return;
        }
    };
    let mut previous = MediaState::default();
    loop {
        tokio::select! {
            changed=state.changed()=>{
                if changed.is_err(){break;}
                let next=state.borrow_and_update().clone();
                if next.device!=previous.device {let _=media.set_device(next.device.as_deref()).await;}
                media.set_ear_detection(next.behavior);
                if next.device.is_some() && (next.ears!=previous.ears || next.behavior!=previous.behavior || next.device!=previous.device) && let Some((left,right))=next.ears {let _=media.update_ears(left,right).await;}
                if next.conversation!=previous.conversation && let Some(activity)=next.conversation {let _=media.conversation_event(activity).await;}
                previous=next;
            },
            _=wait_until(media.next_deadline().map(Instant::from_std))=>{let _=media.tick().await;}
        }
    }
    let _ = media.shutdown().await;
}

struct Cleanup {
    socket: PathBuf,
    state: PathBuf,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.state);
        let _ = fs::remove_file(&self.socket);
    }
}
