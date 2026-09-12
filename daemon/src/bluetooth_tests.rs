//! Exercise the production BlueZ monitor on a private D-Bus daemon.

use super::*;
use std::collections::HashSet;
use std::sync::Mutex;
use zbus::message::Header;

const ADAPTER: &str = "/org/bluez/hci0";
const PODS: &str = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF";
const OTHER: &str = "/org/bluez/hci0/dev_00_11_22_33_44_55";

#[derive(Default)]
struct AdapterState {
    powered: bool,
    owners: HashSet<String>,
    calls: Vec<(String, String)>,
    filters: Vec<(String, String, bool)>,
}

struct TestAdapter(Arc<Mutex<AdapterState>>);

#[zbus::interface(name = "org.bluez.Adapter1")]
impl TestAdapter {
    #[zbus(property)]
    fn powered(&self) -> bool {
        self.0.lock().unwrap().powered
    }

    #[zbus(property)]
    fn discovering(&self) -> bool {
        !self.0.lock().unwrap().owners.is_empty()
    }

    fn set_discovery_filter(&self, filter: Properties, #[zbus(header)] header: Header<'_>) {
        let owner = header.sender().unwrap().to_string();
        let transport = string(&filter, "Transport").unwrap();
        let duplicate = bool::try_from(filter.get("DuplicateData").unwrap()).unwrap();
        self.0
            .lock()
            .unwrap()
            .filters
            .push((owner, transport, duplicate));
    }

    fn start_discovery(&self, #[zbus(header)] header: Header<'_>) -> zbus::fdo::Result<()> {
        let owner = header.sender().unwrap().to_string();
        let mut state = self.0.lock().unwrap();
        state.calls.push((owner.clone(), "StartDiscovery".into()));
        if !state.owners.insert(owner) {
            return Err(zbus::fdo::Error::Failed(
                "Discovery already started by this client".into(),
            ));
        }
        Ok(())
    }

    fn stop_discovery(&self, #[zbus(header)] header: Header<'_>) -> zbus::fdo::Result<()> {
        let owner = header.sender().unwrap().to_string();
        let mut state = self.0.lock().unwrap();
        state.calls.push((owner.clone(), "StopDiscovery".into()));
        if !state.owners.remove(&owner) {
            return Err(zbus::fdo::Error::Failed(
                "No discovery session for this client".into(),
            ));
        }
        Ok(())
    }
}

struct DeviceState {
    address: String,
    alias: String,
    connected: bool,
    uuids: Vec<String>,
    manufacturer: HashMap<u16, Vec<u8>>,
}

struct TestDevice(Arc<Mutex<DeviceState>>);

#[zbus::interface(name = "org.bluez.Device1")]
impl TestDevice {
    #[zbus(property)]
    fn address(&self) -> String {
        self.0.lock().unwrap().address.clone()
    }

    #[zbus(property)]
    fn alias(&self) -> String {
        self.0.lock().unwrap().alias.clone()
    }

    #[zbus(property)]
    fn name(&self) -> String {
        "Bluetooth headset".into()
    }

    #[zbus(property)]
    fn connected(&self) -> bool {
        self.0.lock().unwrap().connected
    }

    #[zbus(property, name = "UUIDs")]
    fn uuids(&self) -> Vec<String> {
        self.0.lock().unwrap().uuids.clone()
    }

    #[zbus(property)]
    fn manufacturer_data(&self) -> HashMap<u16, OwnedValue> {
        self.0
            .lock()
            .unwrap()
            .manufacturer
            .iter()
            .map(|(company, bytes)| (*company, Value::from(bytes.clone()).try_into().unwrap()))
            .collect()
    }
}

struct TestLogin;

#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl TestLogin {
    #[zbus(property)]
    fn preparing_for_sleep(&self) -> bool {
        false
    }
}

async fn property_change(connection: &Connection, path: &str, changed: Properties) {
    connection
        .emit_signal(
            None::<&str>,
            path,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &("org.bluez.Device1", changed, Vec::<String>::new()),
        )
        .await
        .unwrap();
}

async fn prepare_for_sleep(connection: &Connection, sleeping: bool) {
    connection
        .emit_signal(
            None::<&str>,
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
            "PrepareForSleep",
            &(sleeping,),
        )
        .await
        .unwrap();
}

async fn wait_device(
    device: &mut watch::Receiver<Option<Device>>,
    wanted: Option<&str>,
) -> Option<Device> {
    timeout(Duration::from_secs(3), async {
        loop {
            let current = device.borrow_and_update().clone();
            if current.as_ref().map(|device| device.address.as_str()) == wanted {
                return current;
            }
            device.changed().await.unwrap();
        }
    })
    .await
    .expect("BlueZ monitor did not publish the expected active device")
}

async fn wait_scanning(state: &Arc<Mutex<AdapterState>>, owner: &str, expected: bool) {
    timeout(Duration::from_secs(3), async {
        while state.lock().unwrap().owners.contains(owner) != expected {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("BlueZ discovery ownership did not settle");
}

fn advertisement(company: u16, bytes: Vec<u8>) -> Properties {
    HashMap::from([(
        "ManufacturerData".into(),
        OwnedValue::from(HashMap::from([(
            company,
            OwnedValue::try_from(Value::from(bytes)).unwrap(),
        )])),
    )])
}

#[test]
fn selection_keeps_current_airpods_and_falls_back_by_address() {
    let mut cache: Objects = [(PODS, "AA:BB:CC:DD:EE:FF"), (OTHER, "00:11:22:33:44:55")]
        .into_iter()
        .map(|(path, address)| {
            (
                OwnedObjectPath::try_from(path).unwrap(),
                HashMap::from([(
                    "org.bluez.Device1".into(),
                    HashMap::from([
                        ("Address".into(), Value::from(address).try_into().unwrap()),
                        ("Connected".into(), true.into()),
                        (
                            "UUIDs".into(),
                            Value::from(vec![AAP_UUID.to_uppercase()])
                                .try_into()
                                .unwrap(),
                        ),
                    ]),
                )]),
            )
        })
        .collect();
    let pods_path = OwnedObjectPath::try_from(PODS).unwrap();
    let other_path = OwnedObjectPath::try_from(OTHER).unwrap();
    assert_eq!(select_device(&cache, None).unwrap().path, OTHER);
    let current = candidate(&pods_path, cache.get(&pods_path).unwrap()).unwrap();
    cache
        .get_mut(&pods_path)
        .unwrap()
        .get_mut("org.bluez.Device1")
        .unwrap()
        .insert(
            "Alias".into(),
            Value::from("Updated name").try_into().unwrap(),
        );
    let selected = select_device(&cache, Some(&current)).unwrap();
    assert_eq!(selected.path, PODS);
    assert_eq!(selected.name, "Updated name");
    cache
        .get_mut(&pods_path)
        .unwrap()
        .get_mut("org.bluez.Device1")
        .unwrap()
        .insert("Connected".into(), false.into());
    assert_eq!(select_device(&cache, Some(&current)).unwrap().path, OTHER);
    cache.remove(&other_path);
    assert!(select_device(&cache, Some(&current)).is_none());
}

/// Isolate the session bus and environment from parallel tests and the desktop.
#[test]
fn discovery_end_to_end() {
    let output = std::process::Command::new("dbus-run-session")
        .arg("--")
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "bluetooth::tests::discovery_scenario",
            "--ignored",
            "--nocapture",
        ])
        .env("AIRPODS_BLUEZ_TEST", "1")
        .env(
            "DBUS_SYSTEM_BUS_ADDRESS",
            "unix:path=/nonexistent-airpods-test-system",
        )
        .output()
        .expect("dbus-run-session is required for isolated BlueZ lifecycle tests");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[ignore = "launched by discovery_end_to_end inside its own dbus-run-session"]
async fn discovery_scenario() {
    assert_eq!(std::env::var("AIRPODS_BLUEZ_TEST").as_deref(), Ok("1"));
    let adapter = Arc::new(Mutex::new(AdapterState {
        powered: true,
        ..Default::default()
    }));
    let pods = Arc::new(Mutex::new(DeviceState {
        address: "AA:BB:CC:DD:EE:FF".into(),
        alias: "Renamed headphones".into(),
        connected: true,
        uuids: vec![AAP_UUID.to_uppercase()],
        manufacturer: HashMap::new(),
    }));
    let unrelated = Arc::new(Mutex::new(DeviceState {
        address: "00:11:22:33:44:55".into(),
        alias: "AirPods-looking unrelated headset".into(),
        connected: true,
        uuids: vec!["0000110b-0000-1000-8000-00805f9b34fb".into()],
        manufacturer: HashMap::new(),
    }));
    let bluez = zbus::connection::Builder::session()
        .unwrap()
        .name("org.bluez")
        .unwrap()
        .serve_at(ADAPTER, TestAdapter(adapter.clone()))
        .unwrap()
        .serve_at(PODS, TestDevice(pods.clone()))
        .unwrap()
        .serve_at(OTHER, TestDevice(unrelated))
        .unwrap()
        .serve_at("/", zbus::fdo::ObjectManager)
        .unwrap()
        .build()
        .await
        .unwrap();
    let login = zbus::connection::Builder::session()
        .unwrap()
        .name("org.freedesktop.login1")
        .unwrap()
        .serve_at("/org/freedesktop/login1", TestLogin)
        .unwrap()
        .build()
        .await
        .unwrap();
    let rogue = Connection::session().await.unwrap();
    let external_scanner = Connection::session().await.unwrap();
    adapter_call(&external_scanner, ADAPTER, "StartDiscovery")
        .await
        .unwrap();
    let external_owner = external_scanner.unique_name().unwrap().to_string();
    let connection = Connection::session().await.unwrap();
    let owner = connection.unique_name().unwrap().to_string();
    let (device_tx, mut device) = watch::channel(None);
    let (events_tx, mut events) = mpsc::channel(16);
    let (scan, scan_rx) = watch::channel(true);
    let monitor = tokio::spawn(async move {
        super::monitor(&connection, &device_tx, &events_tx, scan_rx)
            .await
            .unwrap();
    });

    // Discover a device that was already connected before the daemon started.
    // Its UUID is authoritative even when its friendly name does not say AirPods.
    let current = wait_device(&mut device, Some("AA:BB:CC:DD:EE:FF"))
        .await
        .unwrap();
    assert_eq!(current.path, PODS);
    assert_eq!(current.name, "Renamed headphones");
    wait_scanning(&adapter, &owner, true).await;
    assert_eq!(adapter.lock().unwrap().owners.len(), 2);
    assert!(
        adapter
            .lock()
            .unwrap()
            .filters
            .contains(&(owner.clone(), "le".into(), true))
    );

    // Name-only devices never substitute for the actual AirPods when they leave.
    pods.lock().unwrap().connected = false;
    property_change(
        &bluez,
        PODS,
        HashMap::from([("Connected".into(), false.into())]),
    )
    .await;
    wait_device(&mut device, None).await;
    pods.lock().unwrap().connected = true;
    property_change(
        &bluez,
        PODS,
        HashMap::from([("Connected".into(), true.into())]),
    )
    .await;
    wait_device(&mut device, Some("AA:BB:CC:DD:EE:FF")).await;

    // D-Bus sender authentication must reject a different process forging BlueZ signals.
    property_change(
        &rogue,
        PODS,
        HashMap::from([("Connected".into(), false.into())]),
    )
    .await;
    property_change(&rogue, PODS, advertisement(0x004c, vec![0x99])).await;
    prepare_for_sleep(&rogue, true).await;
    assert!(
        timeout(Duration::from_millis(150), events.recv())
            .await
            .is_err()
    );
    assert_eq!(
        device.borrow().as_ref().unwrap().address,
        "AA:BB:CC:DD:EE:FF"
    );

    // Only Apple manufacturer data from the real BlueZ name reaches protocol handling.
    property_change(&bluez, PODS, advertisement(0x1234, vec![0x98])).await;
    assert!(
        timeout(Duration::from_millis(100), events.recv())
            .await
            .is_err()
    );
    property_change(&bluez, PODS, advertisement(0x004c, vec![0x07, 0x19, 0x01])).await;
    match timeout(Duration::from_secs(2), events.recv())
        .await
        .unwrap()
        .unwrap()
    {
        Event::Advertisement(address, bytes) => {
            assert_eq!(address, "AA:BB:CC:DD:EE:FF");
            assert_eq!(bytes, [0x07, 0x19, 0x01]);
        }
        _ => panic!("Expected a BlueZ manufacturer-data advertisement"),
    }

    // Stop/start only this client's discovery session, preserving another scanner.
    scan.send_replace(false);
    wait_scanning(&adapter, &owner, false).await;
    assert_eq!(
        adapter.lock().unwrap().owners,
        HashSet::from([external_owner.clone()])
    );
    let stop_count = || {
        adapter
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(caller, method)| caller == &owner && method == "StopDiscovery")
            .count()
    };
    let stops = stop_count();
    property_change(
        &bluez,
        PODS,
        HashMap::from([(
            "Alias".into(),
            Value::from("Fresh alias").try_into().unwrap(),
        )]),
    )
    .await;
    scan.send_replace(false);
    sleep(Duration::from_millis(50)).await;
    assert_eq!(stop_count(), stops);
    scan.send_replace(true);
    wait_scanning(&adapter, &owner, true).await;

    // PrepareForSleep clears the active device and stops only our scan. Resume
    // reloads authoritative properties, including changes that emitted no signal.
    prepare_for_sleep(&login, true).await;
    wait_device(&mut device, None).await;
    wait_scanning(&adapter, &owner, false).await;
    assert!(adapter.lock().unwrap().owners.contains(&external_owner));
    pods.lock().unwrap().alias = "Changed while suspended".into();
    prepare_for_sleep(&login, false).await;
    let resumed = wait_device(&mut device, Some("AA:BB:CC:DD:EE:FF"))
        .await
        .unwrap();
    assert_eq!(resumed.name, "Changed while suspended");
    wait_scanning(&adapter, &owner, true).await;

    // Actual ObjectManager interface lifecycle signals must remove and rediscover
    // the same address without requiring a process restart or a polling interval.
    bluez
        .object_server()
        .remove::<TestDevice, _>(PODS)
        .await
        .unwrap();
    wait_device(&mut device, None).await;
    bluez
        .object_server()
        .at(PODS, TestDevice(pods.clone()))
        .await
        .unwrap();
    wait_device(&mut device, Some("AA:BB:CC:DD:EE:FF")).await;

    scan.send_replace(false);
    wait_scanning(&adapter, &owner, false).await;
    assert!(adapter.lock().unwrap().owners.contains(&external_owner));
    adapter_call(&external_scanner, ADAPTER, "StopDiscovery")
        .await
        .unwrap();
    assert!(adapter.lock().unwrap().owners.is_empty());
    monitor.abort();
    let _ = monitor.await;
}
