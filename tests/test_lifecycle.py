#!/usr/bin/env python3
"""Run the real Rust daemon, control client, and GJS bridge against simulated AAP."""

from contextlib import ExitStack
from pathlib import Path
import json
import os
import shutil
import socket
import struct
import subprocess
import tempfile
import threading
import time
import unittest


PROJECT = Path(__file__).resolve().parent.parent
TARGET = PROJECT / "build/lifecycle"
HANDSHAKE = bytes.fromhex("00000400010002000000000000000000")
HANDSHAKE_ACK = bytes.fromhex("01000400")
FEATURES = bytes.fromhex("040004004d00d700000000000000")
FEATURES_ACK = bytes.fromhex("040004002b00")
NOTIFICATIONS = bytes.fromhex("040004000f00ffffffffff")
CONTROL = bytes.fromhex("040004000900")


def battery_packet(left=81, right=63, case=47):
    components = [(4, left), (2, right)]
    if case is not None:
        components.append((8, case))
    return bytes.fromhex("040004000400") + bytes([len(components)]) + b"".join(
        bytes([kind, 1, level, 2, 1]) for kind, level in components)


def metadata_packet(model="A3048"):
    return bytes.fromhex("040004001d") + bytes(6) + b"Test AirPods\0" + model.encode() + b"\0Apple Inc.\0"


class AirPodsPeer:
    """An independent stream fixture using the documented AAP packet layout."""

    def __init__(self, path):
        self.path = path
        self.listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.listener.bind(str(path))
        self.listener.listen(4)
        self.listener.settimeout(0.1)
        self.stopping = threading.Event()
        self.paused = threading.Event()
        self.lock = threading.Lock()
        self.connection = None
        self.packets = []
        self.errors = []
        self.battery = battery_packet()
        self.metadata = metadata_packet()
        self.reported_controls = ()
        self.echo_controls = True
        self.handshake_drops = 0
        self.echo_noise = True
        self.noise_echo_delay = 0
        self.noise_reports_before_echo = ()
        self.pending_echoes = []
        self.thread = threading.Thread(target=self._serve, daemon=True)
        self.thread.start()

    def _read_exact(self, connection, count):
        data = bytearray()
        while len(data) < count and not self.stopping.is_set():
            try:
                chunk = connection.recv(count - len(data))
            except socket.timeout:
                continue
            if not chunk:
                raise EOFError
            data.extend(chunk)
        if len(data) != count:
            raise EOFError
        return bytes(data)

    def _serve(self):
        while not self.stopping.is_set():
            try:
                connection, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            connection.settimeout(0.1)
            if self.paused.is_set():
                connection.close()
                continue
            with self.lock:
                self.connection = connection
            try:
                while not self.stopping.is_set():
                    length = struct.unpack(">H", self._read_exact(connection, 2))[0]
                    packet = self._read_exact(connection, length)
                    self.packets.append(packet)
                    if packet == HANDSHAKE:
                        if self.handshake_drops:
                            self.handshake_drops -= 1
                        else:
                            self.send(HANDSHAKE_ACK)
                    elif packet == FEATURES:
                        self.send(FEATURES_ACK)
                    elif packet == NOTIFICATIONS:
                        self.send(self.metadata, self.battery,
                                  bytes.fromhex("0400040006000000"), *self.reported_controls)
                    elif packet.startswith(CONTROL) and self.echo_controls:
                        if len(packet) >= 8 and packet[6] == 0x0D:
                            self.send(*self.noise_reports_before_echo)
                            if self.echo_noise:
                                if self.noise_echo_delay:
                                    echo = threading.Thread(target=self._delayed_echo,
                                                            args=(connection, packet), daemon=True)
                                    self.pending_echoes.append(echo)
                                    echo.start()
                                else:
                                    self.send(packet)
                        else:
                            self.send(packet)
            except (EOFError, ConnectionError, OSError):
                pass
            except Exception as error:  # A fixture failure must not masquerade as a daemon bug.
                if not self.stopping.is_set() and not self.paused.is_set():
                    self.errors.append(error)
            finally:
                with self.lock:
                    if self.connection is connection:
                        self.connection = None
                connection.close()

    def send(self, *packets):
        with self.lock:
            if self.connection is None:
                raise RuntimeError("Simulated AirPods are not connected")
            for packet in packets:
                self.connection.sendall(struct.pack(">H", len(packet)) + packet)

    def _delayed_echo(self, connection, packet):
        if self.stopping.wait(self.noise_echo_delay):
            return
        with self.lock:
            # An old peer must not acknowledge a command on a replacement connection.
            if self.connection is connection:
                try:
                    connection.sendall(struct.pack(">H", len(packet)) + packet)
                except OSError:
                    pass

    def disconnect(self):
        self.paused.set()
        with self.lock:
            if self.connection is not None:
                try:
                    self.connection.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
                self.connection.close()
                self.connection = None

    def close(self):
        self.stopping.set()
        self.disconnect()
        self.listener.close()
        self.thread.join(timeout=2)
        for echo in self.pending_echoes:
            echo.join(timeout=2)


class LifecycleTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        subprocess.run([
            "cargo", "build", "--locked", "--features", "test-support", "--bins",
            "--manifest-path", str(PROJECT / "daemon/Cargo.toml"),
            "--target-dir", str(TARGET), "--jobs", os.environ.get("CARGO_BUILD_JOBS", "8"),
        ], cwd=PROJECT, check=True)
        cls.daemon = TARGET / "debug/airpods-gnome"
        cls.ctl = TARGET / "debug/airpods-gnome-ctl"

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ap-life-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for name in ("home", "runtime", "state", "config"):
            (self.root / name).mkdir(mode=0o700)
        self.env = dict(os.environ, HOME=str(self.root / "home"),
                        XDG_RUNTIME_DIR=str(self.root / "runtime"),
                        XDG_STATE_HOME=str(self.root / "state"),
                        XDG_CONFIG_HOME=str(self.root / "config"),
                        DBUS_SESSION_BUS_ADDRESS="unix:path=/nonexistent-airpods-test-session",
                        DBUS_SYSTEM_BUS_ADDRESS="unix:path=/nonexistent-airpods-test-system")
        self.socket_path = self.root / "runtime/airpods-gnome.sock"
        self.status_path = self.root / "state/airpods-gnome/status.json"
        self.config_dir = self.root / "config/AirPodsTrayApp"
        self.log = self.root / "daemon.log"
        self.peer = AirPodsPeer(self.root / "peer.sock")
        self.addCleanup(self.peer.close)
        self.process = None
        self.addCleanup(self.stop)

    def start(self, connected=True):
        with self.log.open("ab") as log:
            self.process = subprocess.Popen([
                str(self.daemon), "--test-transport", str(self.peer.path),
            ], env=self.env, stdout=log, stderr=subprocess.STDOUT)
        self.wait_for(lambda: self.socket_path.exists() and self.status_path.exists())
        if connected:
            self.wait_status(lambda status: status["connected"] and
                             (status["left"]["available"] or status["headset"]["available"]))

    def stop(self):
        if self.process is None:
            return
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        self.process = None

    def wait_for(self, predicate, timeout=7):
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            if self.process is not None and self.process.poll() is not None:
                self.fail(f"Daemon exited {self.process.returncode}:\n{self.log.read_text()}")
            try:
                last = predicate()
                if last:
                    return last
            except (FileNotFoundError, ConnectionError, json.JSONDecodeError):
                pass
            time.sleep(0.02)
        self.fail(f"Lifecycle condition timed out (last={last!r}):\n{self.log.read_text()}")

    def wait_status(self, predicate, timeout=7):
        def matches():
            status = json.loads(self.status_path.read_text())
            return status if predicate(status) else None
        return self.wait_for(matches, timeout)

    def command(self, verb, success=True):
        result = subprocess.run([str(self.ctl), verb], env=self.env,
                                capture_output=True, text=True, timeout=6)
        if success:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout)
            self.assertTrue(result.stderr.strip())
        return result

    def status(self):
        return json.loads(self.command("status").stdout)

    def tearDown(self):
        self.assertFalse(self.peer.errors, self.peer.errors)

    def test_startup_replaces_stale_connected_status_and_uses_private_files(self):
        self.peer.paused.set()
        self.status_path.parent.mkdir()
        self.status_path.write_text('{"schema_version":1,"connected":true}')
        self.start(connected=False)
        status = self.wait_status(lambda status: status["connected"] is False)
        self.assertFalse(status["left"]["available"])
        self.assertFalse(status["case"]["available"])
        self.assertEqual(self.status(), status)
        self.assertEqual(self.status_path.stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.socket_path.stat().st_mode & 0o777, 0o600)

    def test_missing_handshake_reply_reconnects_and_recovers(self):
        self.peer.handshake_drops = 1
        self.start(connected=False)
        self.wait_status(lambda status: status["connected"] and status["left"]["available"])
        self.assertGreaterEqual(self.peer.packets.count(HANDSHAKE), 2)

    def test_independent_batteries_unknown_case_and_ear_notifications(self):
        self.peer.battery = battery_packet(case=None)
        self.start()
        status = self.status()
        self.assertEqual((status["left"]["level"], status["right"]["level"]), (81, 63))
        self.assertFalse(status["case"]["available"])
        self.peer.send(battery_packet(), bytes.fromhex("0400040006000001"))
        status = self.wait_status(lambda status: status["case"]["available"]
                                  and status["left"]["in_ear"] and not status["right"]["in_ear"])
        self.assertEqual(status["case"]["level"], 47)
        self.assertEqual(status["model_number"], "A3048")
        self.assertTrue(status["supports_adaptive"])
        self.assertEqual(status["schema_version"], 1)
        self.peer.send(battery_packet(left=79, right=60, case=None))
        status = self.wait_status(lambda status: status["left"]["level"] == 79)
        self.assertEqual(status["case"]["level"], 47)
        self.assertTrue(status["case"]["available"])

    def test_listening_modes_round_trip_through_wire_protocol(self):
        self.start()
        for command, value in (("noise:off", 0), ("noise:anc", 1),
                               ("noise:transparency", 2), ("noise:adaptive", 3)):
            with self.subTest(command=command):
                self.command(command)
                self.wait_status(lambda status: status["noise_mode"] == value)
                expected = CONTROL + bytes([0x0D, value + 1, 0, 0, 0])
                self.assertIn(expected, self.peer.packets)

    def test_listening_mode_waits_for_matching_confirmation_after_stale_report(self):
        self.peer.reported_controls = (CONTROL + bytes([0x0D, 2, 0, 0, 0]),)
        self.start()
        self.wait_status(lambda status: status["noise_mode"] == 1)
        self.peer.noise_echo_delay = 0.4
        self.peer.noise_reports_before_echo = (
            CONTROL + bytes([0x0D, 3, 0, 0, 0]),  # Earlier Transparency report.
            battery_packet(left=74, right=61, case=46),
        )
        before = time.monotonic()
        self.command("noise:adaptive")
        elapsed = time.monotonic() - before
        self.assertGreaterEqual(elapsed, 0.3, "CLI acknowledged before the requested mode was reported")
        self.assertLess(elapsed, 3)
        status = self.status()
        self.assertEqual(status["noise_mode"], 3)
        self.assertEqual((status["left"]["level"], status["right"]["level"]), (74, 61))

    def test_unconfirmed_listening_mode_keeps_reported_state_and_recovers(self):
        initial_mode = CONTROL + bytes([0x0D, 2, 0, 0, 0])
        self.peer.reported_controls = (initial_mode,)
        self.start()
        self.wait_status(lambda status: status["noise_mode"] == 1)
        self.peer.echo_noise = False
        for reports in ((), (initial_mode,)):
            with self.subTest(response="silent" if not reports else "previous mode repeated"):
                self.peer.noise_reports_before_echo = reports
                before = time.monotonic()
                self.command("noise:adaptive", success=False)
                elapsed = time.monotonic() - before
                self.assertGreaterEqual(elapsed, 1, "Missing confirmation was not awaited")
                self.assertLess(elapsed, 4)
                self.assertEqual(self.status()["noise_mode"], 1)
                self.command("ear:both")
                self.wait_status(lambda status: status["ear_detection_behavior"] == 1)
        self.peer.echo_noise = True
        self.peer.noise_reports_before_echo = ()
        self.command("noise:transparency")
        self.wait_status(lambda status: status["noise_mode"] == 2)

    def test_airpods_pro_3_hides_off_mode_and_supports_adaptive_controls(self):
        self.peer.metadata = metadata_packet("A3064")
        self.start()
        status = self.status()
        self.assertFalse(status["supports_noise_off"])
        self.assertTrue(status["supports_adaptive"])
        self.assertTrue(status["supports_conversational_awareness"])
        self.command("noise:off", success=False)
        for command in ("noise:adaptive", "adaptive:100", "ca:on"):
            self.command(command)
        self.wait_status(lambda status: status["noise_mode"] == 3 and status["adaptive_noise_level"] == 100
                         and status["conversational_awareness"])
        self.assertNotIn(CONTROL + bytes([0x0D, 1, 0, 0, 0]), self.peer.packets)

    def test_airpods_max_publishes_one_headset_battery(self):
        self.peer.metadata = metadata_packet("A3184")
        self.peer.battery = bytes.fromhex("040004000400010101550201")
        self.start()
        status = self.status()
        self.assertTrue(status["is_headset"])
        self.assertEqual(status["headset"]["level"], 85)
        self.assertFalse(status["left"]["available"])
        self.assertFalse(status["right"]["available"])
        self.assertFalse(status["case"]["available"])
        self.command("noise:adaptive", success=False)
        self.command("noise:anc")

    def test_fresh_preferences_survive_restart(self):
        self.assertFalse(self.config_dir.exists())
        self.start()
        self.assertEqual(self.status()["ear_detection_behavior"], 0)
        for verb in ("noise:adaptive", "ear:both", "ca:on", "adaptive:73"):
            self.command(verb)
        saved = self.wait_status(lambda status: status["ear_detection_behavior"] == 1
                                 and status["conversational_awareness"]
                                 and status["adaptive_noise_level"] == 73)
        self.stop()
        self.start()
        status = self.wait_status(lambda status: status["ear_detection_behavior"] == 1
                                  and status["conversational_awareness"]
                                  and status["adaptive_noise_level"] == 73)
        for key in ("ear_detection_behavior", "conversational_awareness", "adaptive_noise_level"):
            self.assertEqual(status[key], saved[key])
        self.assertEqual((self.config_dir / "rust-settings.json").stat().st_mode & 0o777, 0o600)

    def test_disconnect_publishes_hidden_state_and_reconnects_with_fresh_batteries(self):
        self.start()
        self.peer.disconnect()
        self.wait_status(lambda status: status["connected"] is False)
        self.command("noise:anc", success=False)
        self.peer.battery = battery_packet(left=21, right=69, case=45)
        self.peer.paused.clear()
        status = self.wait_status(lambda status: status["connected"] and status["left"]["level"] == 21)
        self.assertEqual(status["right"]["level"], 69)
        self.assertEqual(status["case"]["level"], 45)
        self.assertGreaterEqual(self.peer.packets.count(HANDSHAKE), 2)

    def test_airpods_reports_override_remembered_controls_after_restart(self):
        self.start()
        for verb in ("noise:adaptive", "ear:both", "ca:on", "adaptive:73"):
            self.command(verb)
        self.wait_status(lambda status: status["conversational_awareness"]
                         and status["adaptive_noise_level"] == 73)
        self.stop()
        # A phone changed these hardware controls while the backend was stopped.
        self.peer.reported_controls = (
            CONTROL + bytes([0x0D, 3, 0, 0, 0]),  # Transparency.
            CONTROL + bytes([0x28, 2, 0, 0, 0]),  # Conversation Awareness off.
            CONTROL + bytes([0x2E, 42, 0, 0, 0]),
        )
        self.start()
        status = self.wait_status(lambda status: status["noise_mode"] == 2
                                  and not status["conversational_awareness"]
                                  and status["adaptive_noise_level"] == 42)
        self.assertEqual(status["ear_detection_behavior"], 1)

    def test_malformed_packets_do_not_corrupt_state_or_stop_updates(self):
        self.start()
        invalid = (b"", b"\x04", bytes.fromhex("040004000400"),
                   bytes.fromhex("040004000400030401510201"),
                   bytes.fromhex("04000400060000"),
                   CONTROL + bytes([0x0D, 255, 0, 0, 0]),
                   bytes.fromhex("040004001d") + b"\0\0",
                   b"\xff" * 512)
        self.peer.send(*invalid, battery_packet(left=82, right=62, case=46))
        status = self.wait_status(lambda status: status["left"]["level"] == 82)
        self.assertEqual(status["right"]["level"], 62)
        self.assertEqual(status["case"]["level"], 46)
        self.assertEqual(status["model_number"], "A3048")
        self.command("noise:anc")

    def test_bad_commands_fail_promptly_and_next_command_still_works(self):
        self.start()
        for command in ("unknown", "adaptive:-1", "adaptive:101", "noise:invalid", "ear:one\nstatus"):
            with self.subTest(command=command):
                before = time.monotonic()
                self.command(command, success=False)
                self.assertLess(time.monotonic() - before, 5)
        self.command("ear:one")
        self.wait_status(lambda status: status["ear_detection_behavior"] == 0)

    def test_second_daemon_is_rejected_without_damaging_running_instance(self):
        self.start()
        second = subprocess.run([str(self.daemon), "--test-transport", str(self.peer.path)],
                                env=self.env, capture_output=True, text=True, timeout=5)
        self.assertNotEqual(second.returncode, 0)
        self.assertIn("already running", second.stderr)
        self.assertTrue(self.status()["connected"])
        self.assertIsNone(self.process.poll())

    def test_sigterm_cleans_socket_and_connected_state(self):
        self.start()
        process = self.process
        self.stop()
        self.assertEqual(process.returncode, 0, self.log.read_text())
        self.assertFalse(self.socket_path.exists())
        if self.status_path.exists():
            self.assertFalse(json.loads(self.status_path.read_text())["connected"])
        self.command("status", success=False)

    def test_restart_after_crash_recovers_stale_socket_and_state(self):
        self.start()
        self.process.kill()
        self.process.wait(timeout=5)
        self.process = None
        self.peer.disconnect()
        self.start(connected=False)
        self.wait_status(lambda status: status["connected"] is False)
        self.peer.paused.clear()
        self.wait_status(lambda status: status["connected"])
        self.command("noise:transparency")

    def test_abandoned_and_oversize_clients_do_not_block_status_or_commands(self):
        self.start()
        with ExitStack() as clients:
            for _ in range(12):
                client = clients.enter_context(socket.socket(socket.AF_UNIX, socket.SOCK_STREAM))
                client.settimeout(4)
                client.connect(str(self.socket_path))
                client.sendall(b"noise:")
            self.command("status")
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as oversized:
                oversized.settimeout(4)
                oversized.connect(str(self.socket_path))
                oversized.sendall(b"x" * 4096 + b"\n")
                try:
                    response = oversized.recv(1024)
                    self.assertTrue(not response or response.startswith(b"error:"), response)
                except ConnectionResetError:
                    pass
            self.command("ear:both")
            self.wait_status(lambda status: status["ear_detection_behavior"] == 1)

    @unittest.skipUnless(shutil.which("gjs"), "GJS is required for the extension bridge check")
    def test_gjs_backend_observes_disconnect_and_reconnect(self):
        self.start()
        script = self.root / "bridge-lifecycle.js"
        marker = self.root / "bridge-stage"
        script.write_text("""
import GLib from 'gi://GLib';
import {Backend} from %s;
const loop = new GLib.MainLoop(null, false);
let stage = 0;
let failure = null;
const backend = new Backend(status => {
    if ((stage === 0 && status.connected) || (stage === 1 && !status.connected) ||
        (stage === 2 && status.connected)) {
        stage++;
        GLib.file_set_contents(%s, String(stage));
        if (stage === 3) loop.quit();
    }
}, message => { failure = message; loop.quit(); });
const timeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 18000, () => {
    failure = 'Missing connection lifecycle update';
    loop.quit();
    return GLib.SOURCE_REMOVE;
});
loop.run();
backend.destroy();
if (stage === 3) GLib.Source.remove(timeout);
if (failure || stage !== 3) throw new Error(failure ?? 'Incomplete connection lifecycle');
""" % (json.dumps((PROJECT / "backend.js").as_uri()), json.dumps(str(marker))))
        with subprocess.Popen(["gjs", "-m", str(script)], env=self.env,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as bridge:
            try:
                self.wait_for(lambda: marker.read_text() == "1")
                self.peer.disconnect()
                self.wait_for(lambda: marker.read_text() == "2")
                self.peer.paused.clear()
                output, error = bridge.communicate(timeout=10)
                self.assertEqual(bridge.returncode, 0, output + error)
                self.assertEqual(marker.read_text(), "3")
            finally:
                if bridge.poll() is None:
                    bridge.kill()
                    bridge.communicate(timeout=5)

    @unittest.skipUnless(shutil.which("gjs"), "GJS is required for the extension bridge check")
    def test_gjs_backend_reads_live_state_and_delivers_latest_queued_choices(self):
        self.start()
        script = self.root / "bridge.js"
        script.write_text("""
import GLib from 'gi://GLib';
import {Backend} from %s;
const loop = new GLib.MainLoop(null, false);
let sent = false;
let complete = false;
let failure = null;
const backend = new Backend(status => {
    if (!sent && status.connected) {
        sent = true;
        for (const command of ['noise:adaptive', 'ear:both', 'ca:on', 'adaptive:27', 'adaptive:83'])
            backend.command(command);
    }
    if (sent && status.mode === 3 && status.ear === 1 && status.conversation && status.adaptive === 83) {
        complete = true;
        loop.quit();
    }
}, message => { failure = message; loop.quit(); }, GLib.getenv('XDG_STATE_HOME'), {ctlPath: %s});
const timeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 8000, () => {
    failure = 'GJS bridge did not observe the final choices';
    loop.quit();
    return GLib.SOURCE_REMOVE;
});
loop.run();
backend.destroy();
if (complete) GLib.Source.remove(timeout);
if (failure || !complete) throw new Error(failure ?? 'No final status');
print('Live Rust daemon and GJS bridge passed');
""" % (json.dumps((PROJECT / "backend.js").as_uri()), json.dumps(str(self.ctl))))
        result = subprocess.run(["gjs", "-m", str(script)], env=self.env,
                                capture_output=True, text=True, timeout=12)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr + self.log.read_text())
        self.assertIn("Live Rust daemon and GJS bridge passed", result.stdout)


if __name__ == "__main__":
    unittest.main(verbosity=2)
