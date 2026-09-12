// SPDX-License-Identifier: GPL-3.0-or-later
//! BlueZ discovery and a bounded, reconnecting AAP link. No UI dependencies.
use anyhow::{Context, Result};
use bluer::{
    AddressType,
    l2cap::{SeqPacket, SocketAddr},
};
use futures_util::StreamExt;
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    sync::{mpsc, watch},
    task::JoinSet,
    time::{sleep, timeout},
};
use zbus::{
    Connection, MatchRule, MessageStream, Proxy,
    zvariant::{Array, Dict, OwnedObjectPath, OwnedValue, Value},
};

const AAP_UUID: &str = "74ec2172-0bad-4d01-8f77-997b2be0722a";
type Properties = HashMap<String, OwnedValue>;
type Interfaces = HashMap<String, Properties>;
type Objects = HashMap<OwnedObjectPath, Interfaces>;

#[cfg(test)]
#[path = "bluetooth_tests.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub address: String,
    pub name: String,
    pub path: String,
}

pub enum Event {
    Connected(Device),
    Disconnected,
    Packet(Vec<u8>),
    Advertisement(String, Vec<u8>),
}

enum Link {
    Bluetooth(SeqPacket),
    #[cfg(feature = "test-support")]
    Test(tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>),
}

impl Link {
    async fn recv(&self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Bluetooth(socket) => socket.recv(buffer).await,
            #[cfg(feature = "test-support")]
            Self::Test(_) => Err(std::io::Error::other(
                "Test reader is owned by its fixture task",
            )),
        }
    }
    async fn close(&self) {
        match self {
            Self::Bluetooth(socket) => {
                let _ = socket.shutdown(std::net::Shutdown::Both);
            }
            #[cfg(feature = "test-support")]
            Self::Test(writer) => {
                use tokio::io::AsyncWriteExt;
                let _ = writer.lock().await.shutdown().await;
            }
        }
    }
    async fn send(&self, packet: &[u8]) -> Result<()> {
        match self {
            Self::Bluetooth(socket) => {
                anyhow::ensure!(
                    socket.send(packet).await? == packet.len(),
                    "Incomplete Bluetooth write"
                );
            }
            #[cfg(feature = "test-support")]
            Self::Test(writer) => {
                use tokio::io::AsyncWriteExt;
                let mut writer = writer.lock().await;
                writer.write_u16(packet.len().try_into()?).await?;
                writer.write_all(packet).await?;
            }
        }
        Ok(())
    }
}

pub struct Bluetooth {
    link: watch::Receiver<Option<Arc<Link>>>,
    device: watch::Receiver<Option<Device>>,
    tasks: JoinSet<()>,
    connection: Option<Connection>,
    scan: watch::Sender<bool>,
}

impl Bluetooth {
    pub async fn start(events: mpsc::Sender<Event>, test_socket: Option<PathBuf>) -> Result<Self> {
        let (link_tx, link) = watch::channel(None);
        let (device_tx, device) = watch::channel(None);
        let (scan, scan_rx) = watch::channel(true);
        let mut tasks = JoinSet::new();
        #[cfg(feature = "test-support")]
        if let Some(path) = test_socket {
            let device_value = Device {
                address: "00:11:22:33:44:55".into(),
                name: "AirPods".into(),
                path: String::new(),
            };
            device_tx.send_replace(Some(device_value.clone()));
            tasks.spawn(test_loop(path, device_value, link_tx, events));
            return Ok(Self {
                link,
                device,
                tasks,
                connection: None,
                scan,
            });
        }
        #[cfg(not(feature = "test-support"))]
        anyhow::ensure!(
            test_socket.is_none(),
            "Test transport is not included in this build"
        );
        let connection = Connection::system()
            .await
            .context("Connect to system D-Bus")?;
        let conn = connection.clone();
        let event_tx = events.clone();
        tasks.spawn(async move {
            loop {
                if let Err(error) = monitor(&conn, &device_tx, &event_tx, scan_rx.clone()).await {
                    eprintln!("Bluetooth discovery: {error}");
                    device_tx.send_replace(None);
                }
                if device_tx.is_closed() {
                    break;
                }
                sleep(Duration::from_secs(3)).await;
            }
        });
        tasks.spawn(control_loop(device.clone(), link_tx, events, scan.clone()));
        Ok(Self {
            link,
            device,
            tasks,
            connection: Some(connection),
            scan,
        })
    }

    pub async fn send(&self, packet: &[u8]) -> Result<()> {
        let link = self
            .link
            .borrow()
            .clone()
            .context("AirPods are disconnected")?;
        timeout(Duration::from_secs(2), link.send(packet))
            .await
            .context("Bluetooth command timed out")??;
        Ok(())
    }

    pub async fn reconnect(&self) {
        let link = self.link.borrow().clone();
        if let Some(link) = link {
            link.close().await;
        }
    }

    pub async fn device_command(&self, method: &str, remembered: Option<&str>) -> Result<()> {
        let connection = self
            .connection
            .as_ref()
            .context("Bluetooth management unavailable")?;
        let current = self.device.borrow().clone();
        let path = if let Some(device) = current {
            device.path
        } else {
            objects(connection)
                .await?
                .into_iter()
                .find_map(|(path, interfaces)| {
                    let props = interfaces.get("org.bluez.Device1")?;
                    let address = string(props, "Address")?;
                    if remembered.is_some_and(|saved| {
                        !saved.is_empty() && !address.eq_ignore_ascii_case(saved)
                    }) {
                        return None;
                    }
                    has_aap_service(props).then(|| path.to_string())
                })
                .context("No paired AirPods found")?
        };
        let device =
            Proxy::new(connection, "org.bluez", path.as_str(), "org.bluez.Device1").await?;
        timeout(Duration::from_secs(2), device.call_method(method, &())).await??;
        Ok(())
    }

    pub async fn shutdown(&mut self) {
        self.scan.send_replace(false);
        self.tasks.abort_all();
        while self.tasks.join_next().await.is_some() {}
        if let Some(connection) = &self.connection {
            // Stop only our discovery session; BlueZ reference-counts other clients.
            if let Ok(objects) = objects(connection).await {
                for (path, interfaces) in objects {
                    if interfaces.contains_key("org.bluez.Adapter1") {
                        let _ = adapter_call(connection, path.as_str(), "StopDiscovery").await;
                    }
                }
            }
        }
    }
}

async fn control_loop(
    mut device: watch::Receiver<Option<Device>>,
    link: watch::Sender<Option<Arc<Link>>>,
    events: mpsc::Sender<Event>,
    scan: watch::Sender<bool>,
) {
    let mut failures = 0u32;
    loop {
        let current = device.borrow().clone();
        let Some(current) = current else {
            if device.changed().await.is_err() {
                break;
            }
            continue;
        };
        scan.send_replace(false);
        let connect = async {
            let address = current.address.parse()?;
            let socket =
                SeqPacket::connect(SocketAddr::new(address, AddressType::BrEdr, 0x1001)).await?;
            Ok::<_, anyhow::Error>(Arc::new(Link::Bluetooth(socket)))
        };
        let connection = tokio::select! {
            result = timeout(Duration::from_secs(10), connect) => result.map_err(anyhow::Error::from).and_then(|r| r),
            _ = device.changed() => { continue; }
        };
        match connection {
            Ok(socket) => {
                failures = 0;
                link.send_replace(Some(socket.clone()));
                if events
                    .send(Event::Connected(current.clone()))
                    .await
                    .is_err()
                {
                    break;
                }
                scan.send_replace(true);
                let mut buffer = vec![0u8; 65_536];
                loop {
                    tokio::select! {
                        result = socket.recv(&mut buffer) => {
                            match result {
                                Ok(0) | Err(_) => break,
                                Ok(size) => { if events.send(Event::Packet(buffer[..size].to_vec())).await.is_err() { return; } }
                            }
                        }
                        result = device.changed() => {
                            if result.is_err() || device.borrow().as_ref().map(|d| &d.address) != Some(&current.address) { break; }
                        }
                    }
                }
                link.send_replace(None);
                let _ = events.send(Event::Disconnected).await;
            }
            Err(error) => {
                failures = failures.saturating_add(1);
                if failures == 1 {
                    eprintln!("AirPods control connection: {error}");
                }
            }
        }
        scan.send_replace(true);
        let delay = Duration::from_secs(1 << failures.min(4));
        tokio::select! { _ = sleep(delay) => {}, _ = device.changed() => { failures=0; } }
    }
}

fn string(properties: &Properties, key: &str) -> Option<String> {
    properties
        .get(key)
        .and_then(|v| <&str>::try_from(v).ok())
        .map(str::to_owned)
}

fn has_aap_service(properties: &Properties) -> bool {
    let Some(uuids) = properties
        .get("UUIDs")
        .and_then(|value| <&Array>::try_from(value).ok())
    else {
        return false;
    };
    uuids
        .iter()
        .try_fold(false, |found, value| {
            value
                .downcast_ref::<&str>()
                .map(|uuid| found || uuid.eq_ignore_ascii_case(AAP_UUID))
        })
        .unwrap_or(false)
}

fn candidate(path: &OwnedObjectPath, interfaces: &Interfaces) -> Option<Device> {
    let properties = interfaces.get("org.bluez.Device1")?;
    if !properties
        .get("Connected")
        .and_then(|v| bool::try_from(v).ok())
        .unwrap_or(false)
    {
        return None;
    }
    if !has_aap_service(properties) {
        return None;
    }
    Some(Device {
        address: string(properties, "Address")?,
        name: string(properties, "Alias")
            .or_else(|| string(properties, "Name"))
            .unwrap_or_else(|| "AirPods".into()),
        path: path.to_string(),
    })
}

fn select_device(cache: &Objects, current: Option<&Device>) -> Option<Device> {
    let mut selected: Option<Device> = None;
    for device in cache
        .iter()
        .filter_map(|(path, interfaces)| candidate(path, interfaces))
    {
        if current.is_some_and(|current| current.address == device.address) {
            return Some(device);
        }
        if selected
            .as_ref()
            .is_none_or(|selected| device.address < selected.address)
        {
            selected = Some(device);
        }
    }
    selected
}

async fn objects(connection: &Connection) -> Result<Objects> {
    let proxy = Proxy::new(
        connection,
        "org.bluez",
        "/",
        "org.freedesktop.DBus.ObjectManager",
    )
    .await?;
    Ok(timeout(Duration::from_secs(2), proxy.call("GetManagedObjects", &())).await??)
}

async fn adapter_call(connection: &Connection, path: &str, method: &str) -> Result<()> {
    let proxy = Proxy::new(connection, "org.bluez", path, "org.bluez.Adapter1").await?;
    timeout(Duration::from_secs(2), proxy.call_method(method, &())).await??;
    Ok(())
}

async fn monitor(
    connection: &Connection,
    device: &watch::Sender<Option<Device>>,
    events: &mpsc::Sender<Event>,
    mut scan: watch::Receiver<bool>,
) -> Result<()> {
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.bluez")?
        .build();
    let mut messages = MessageStream::for_match_rule(rule, connection, Some(256)).await?;
    let sleep_rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.login1")?
        .interface("org.freedesktop.login1.Manager")?
        .member("PrepareForSleep")?
        .build();
    let mut sleep_messages = MessageStream::for_match_rule(sleep_rule, connection, Some(4)).await?;
    let mut cache = objects(connection).await?;
    let mut scans = HashMap::<String, bool>::new();
    let mut sleeping = false;
    let mut timer = tokio::time::interval(Duration::from_secs(10));
    loop {
        let wanted = if sleeping {
            None
        } else {
            let current = device.borrow().clone();
            select_device(&cache, current.as_ref())
        };
        device.send_if_modified(|value| {
            if *value == wanted {
                false
            } else {
                *value = wanted;
                true
            }
        });
        let want_scan = *scan.borrow() && !sleeping;
        for (path, interfaces) in &cache {
            let Some(properties) = interfaces.get("org.bluez.Adapter1") else {
                continue;
            };
            let powered = properties
                .get("Powered")
                .and_then(|v| bool::try_from(v).ok())
                .unwrap_or(false);
            let enabled = want_scan && powered;
            if scans.get(path.as_str()) == Some(&enabled) {
                continue;
            }
            let result = if enabled {
                let proxy =
                    Proxy::new(connection, "org.bluez", path.as_str(), "org.bluez.Adapter1")
                        .await?;
                let filter = HashMap::from([
                    ("Transport", Value::from("le")),
                    ("DuplicateData", Value::from(true)),
                ]);
                let _ = timeout(
                    Duration::from_secs(2),
                    proxy.call_method("SetDiscoveryFilter", &(filter,)),
                )
                .await;
                adapter_call(connection, path.as_str(), "StartDiscovery").await
            } else {
                adapter_call(connection, path.as_str(), "StopDiscovery").await
            };
            if result.is_ok() || !enabled {
                scans.insert(path.to_string(), enabled);
            }
        }
        tokio::select! {
            _=scan.changed()=>{},
            _=timer.tick()=>{cache=objects(connection).await?;},
            message=sleep_messages.next()=>{
                if let Some(Ok(message))=message && let Ok((value,))=message.body().deserialize::<(bool,)>() {sleeping=value;if !sleeping {cache=objects(connection).await?;}}
            },
            message=messages.next()=>{
                let message=message.context("BlueZ event stream closed")??;
                let header=message.header();
                let member=header.member().map(|s|s.as_str()).unwrap_or("");
                match member {
                    "PropertiesChanged"=>{
                        let Some(path)=header.path() else {continue};
                        if let Ok((interface,changed,invalidated))=message.body().deserialize::<(String,Properties,Vec<String>)>() {
                            let entry=cache.entry(path.to_owned().into()).or_default().entry(interface.clone()).or_default();
                            let advertisement=changed.contains_key("ManufacturerData");
                            entry.extend(changed);
                            for key in invalidated {entry.remove(&key);}
                            if advertisement && interface=="org.bluez.Device1" {emit_advertisement(entry,events).await;}
                        }
                    },
                    "InterfacesAdded"=>{
                        if let Ok((path,interfaces))=message.body().deserialize::<(OwnedObjectPath,Interfaces)>() {
                            if let Some(properties)=interfaces.get("org.bluez.Device1") {emit_advertisement(properties,events).await;}
                            cache.entry(path).or_default().extend(interfaces);
                        }
                    },
                    "InterfacesRemoved"=>{
                        if let Ok((path,names))=message.body().deserialize::<(OwnedObjectPath,Vec<String>)>() {
                            if let Some(interfaces)=cache.get_mut(&path) {for name in names {interfaces.remove(&name);}}
                            scans.remove(path.as_str());
                        }
                    },
                    _=>{}
                }
            }
        }
    }
}

async fn emit_advertisement(properties: &Properties, events: &mpsc::Sender<Event>) {
    let Some(data) = properties
        .get("ManufacturerData")
        .and_then(|value| <&Dict>::try_from(value).ok())
    else {
        return;
    };
    if let Ok(Some(bytes)) = data.get::<_, &Array>(&0x004c_u16)
        && let Ok(bytes) = bytes
            .iter()
            .map(|value| value.downcast_ref::<u8>())
            .collect::<std::result::Result<Vec<_>, _>>()
        && let Some(address) = string(properties, "Address")
    {
        let _ = events.send(Event::Advertisement(address, bytes)).await;
    }
}

#[cfg(feature = "test-support")]
async fn test_loop(
    path: PathBuf,
    device: Device,
    link: watch::Sender<Option<Arc<Link>>>,
    events: mpsc::Sender<Event>,
) {
    use tokio::{io::AsyncReadExt, net::UnixStream};
    loop {
        if let Ok(socket) = UnixStream::connect(&path).await {
            let (mut reader, writer) = socket.into_split();
            link.send_replace(Some(Arc::new(Link::Test(tokio::sync::Mutex::new(writer)))));
            if events.send(Event::Connected(device.clone())).await.is_err() {
                break;
            }
            while let Ok(size) = reader.read_u16().await {
                let mut packet = vec![0; usize::from(size)];
                if reader.read_exact(&mut packet).await.is_err() {
                    break;
                }
                if events.send(Event::Packet(packet)).await.is_err() {
                    return;
                }
            }
            link.send_replace(None);
            let _ = events.send(Event::Disconnected).await;
        }
        sleep(Duration::from_millis(300)).await;
    }
}
