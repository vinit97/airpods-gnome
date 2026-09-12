#!/usr/bin/env python3
"""Exercise setup failures and replacement ordering without touching the desktop."""

from pathlib import Path
import ast
import json
import os
import shutil
import subprocess
import tempfile
import unittest


PROJECT = Path(__file__).resolve().parent.parent
MOCK_TOOL = r'''#!/usr/bin/env python3
from pathlib import Path
import json
import os
import shutil
import sys
from zipfile import ZipFile

name = Path(sys.argv[0]).name
args = sys.argv[1:]
root = Path(os.environ["INSTALLER_TEST_ROOT"])
project = root / "project with spaces"
state_path = root / "state.json"
state = json.loads(state_path.read_text())
event = {"tool": name, "args": args}
if name == "systemctl":
    service_name = args[-1] if args[-1].endswith(".service") else "airpods-gnome.service"
    assert service_name == "airpods-gnome.service"
    binary = root / "desktop home/.local/bin" / service_name.removesuffix(".service")
    event["binary"] = binary.read_text() if binary.exists() else None
with (root / "events.jsonl").open("a") as events:
    events.write(json.dumps(event) + "\n")

failure = os.environ.get("INSTALLER_TEST_FAIL", "")
if name == "cargo":
    assert "--locked" in args and "--release" in args
    assert args[args.index("--manifest-path") + 1] == str(project / "daemon/Cargo.toml")
    if args[0] == "build":
        if failure == "build":
            sys.exit(13)
        output = Path(args[args.index("--target-dir") + 1]) / "release"
        for binary in ("airpods-gnome", "airpods-gnome-ctl"):
            target = output / binary
            if failure == "incomplete-stage" and binary == "airpods-gnome-ctl":
                target.unlink(missing_ok=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text("new backend")
            target.chmod(0o755)
    elif args[0] == "test" and failure == "tests":
        sys.exit(15)
elif name == "install":
    source, target = map(Path, args[-2:])
    staging = "airpods-gnome-install." in str(target)
    if staging and failure == "stage":
        sys.exit(14)
    if not staging and failure == "replace" and target.name == "airpods-gnome-ctl":
        sys.exit(22)
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, target)
    target.chmod(int(args[args.index("-m") + 1], 8))
elif name == "gnome-extensions":
    if args[0] == "pack":
        if failure == "pack":
            sys.exit(16)
        output = Path(next(arg.removeprefix("--out-dir=") for arg in args if arg.startswith("--out-dir=")))
        output.mkdir(parents=True, exist_ok=True)
        uuid = json.loads((project / "metadata.json").read_text())["uuid"]
        with ZipFile(output / (uuid + ".shell-extension.zip"), "w") as archive:
            for relative in ("extension.js", "model.js", "backend.js", "stylesheet.css", "metadata.json", "LICENSE"):
                archive.write(project / relative, relative)
            for icon in (project / "icons").iterdir():
                archive.write(icon, "icons/" + icon.name)
            if failure == "bad-package":
                archive.writestr("daemon/private-source.cpp", "must not be bundled")
    elif args[0] == "install" and failure == "extension-install":
        sys.exit(17)
elif name == "gjs":
    assert args[0] == "-c"
    assert args[-1] == json.loads((project / "metadata.json").read_text())["uuid"]
    if failure == "extension-enable":
        sys.exit(23)
elif name == "systemctl":
    verb = args[1]
    if verb == "show-environment" and failure == "session":
        sys.exit(18)
    elif verb == "is-active":
        sys.exit(0 if state["active"] else 3)
    elif verb == "is-enabled":
        sys.exit(0 if state["enabled"] else 1)
    elif verb == "show":
        assert "--property=LoadState" in args
        print("loaded" if state["exists"] else "not-found")
    elif verb == "stop":
        state["active"] = False
        marker = root / "stop-failed"
        if failure == "service-stop" and not marker.exists():
            marker.touch()
            state_path.write_text(json.dumps(state))
            sys.exit(21)
    elif verb == "enable":
        state["enabled"] = True
        state["exists"] = True
        if failure in ("service-start", "service-recovery"):
            state_path.write_text(json.dumps(state))
            sys.exit(19)
        if "--now" in args:
            state["active"] = True
    elif verb == "disable":
        state["enabled"] = False
    elif verb == "start":
        if failure == "service-recovery":
            sys.exit(20)
        state["active"] = True
    elif verb == "daemon-reload":
        unit_directory = root / "desktop home/.local/share/systemd/user"
        state["exists"] = (unit_directory / "airpods-gnome.service").exists()
    state_path.write_text(json.dumps(state))
'''


class SetupTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="airpods-setup-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.project = self.root / "project with spaces"
        self.project.mkdir()
        self.home = self.root / "desktop home"
        self.prefix = self.home / ".local"
        self.state_path = self.root / "state.json"
        self.env = dict(os.environ, HOME=str(self.home), TMPDIR=str(self.root),
                        XDG_CONFIG_HOME=str(self.home / ".config"),
                        XDG_DATA_HOME=str(self.home / "custom data"),
                        XDG_CACHE_HOME=str(self.home / ".cache"),
                        INSTALLER_TEST_ROOT=str(self.root))
        for relative in ("setup", "metadata.json", "extension.js", "model.js", "backend.js",
                         "stylesheet.css", "LICENSE", "scripts/common.sh",
                         "scripts/build-backend.sh", "scripts/pack.sh", "scripts/check-package.py"):
            target = self.project / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(PROJECT / relative, target)
        shutil.copytree(PROJECT / "icons", self.project / "icons")
        (self.project / "daemon").mkdir()
        (self.project / "daemon/Cargo.toml").write_text("# mock backend\n")
        (self.project / "daemon/Cargo.lock").write_text("# mock lockfile\n")
        (self.project / "daemon/airpods-gnome.service").write_text("new unit")
        for relative in ("bin/airpods-gnome", "bin/airpods-gnome-ctl", "share/systemd/user/airpods-gnome.service"):
            target = self.prefix / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text("old backend" if relative.startswith("bin/") else "old unit")
            target.chmod(0o755 if relative.startswith("bin/") else 0o644)
        self.config = self.home / ".config/AirPodsTrayApp/rust-settings.json"
        self.config.parent.mkdir(parents=True)
        self.config.write_text('{"ear_detection_behavior":1}\n')
        self.write_state(active=True, enabled=True, exists=True)
        commands = self.root / "mock-bin"
        commands.mkdir()
        for name in ("cargo", "rustc", "gnome-extensions", "systemctl",
                     "install", "wpctl", "pw-dump", "gjs"):
            tool = commands / name
            tool.write_text(MOCK_TOOL)
            tool.chmod(0o755)
        self.env["PATH"] = str(commands) + os.pathsep + os.environ["PATH"]
        self.env.pop("CARGO_BUILD_JOBS", None)

    def run_setup(self, *args, failure="", expected=0):
        settings = self.config.read_bytes() if self.config.exists() else None
        result = subprocess.run([str(self.project / "setup"), *args], cwd=self.root,
                                env=self.env | {"INSTALLER_TEST_FAIL": failure},
                                capture_output=True, text=True, timeout=30)
        if expected is None:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        else:
            self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        self.assertEqual(self.config.read_bytes() if self.config.exists() else None, settings)
        return result

    def state(self):
        return json.loads(self.state_path.read_text()) if self.state_path.exists() else {}

    def write_state(self, **values):
        self.state_path.write_text(json.dumps(self.state() | values))

    def events(self):
        log = self.root / "events.jsonl"
        return [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []

    def commands(self):
        return [(event["tool"], event["args"][1 if event["tool"] == "systemctl" else 0])
                for event in self.events()]

    def assert_old_backend(self):
        self.assertEqual((self.prefix / "bin/airpods-gnome").read_text(), "old backend")
        self.assertEqual((self.prefix / "bin/airpods-gnome-ctl").read_text(), "old backend")
        self.assertEqual((self.prefix / "share/systemd/user/airpods-gnome.service").read_text(), "old unit")

    def test_build_only_leaves_installation_and_service_untouched(self):
        self.run_setup("--build-only")
        self.assert_old_backend()
        self.assertFalse(any(event["tool"] == "gjs" for event in self.events()))
        self.assertFalse(any(event["tool"] == "systemctl" for event in self.events()))
        self.assertNotIn(("gnome-extensions", "install"), self.commands())
        self.assertTrue(list((self.project / "dist").glob("*.zip")))

    def test_successful_install_enables_extension_after_backend_start(self):
        self.run_setup()
        start = self.commands().index(("systemctl", "enable"))
        enable = self.commands().index(("gjs", "-c"))
        self.assertLess(start, enable)

    def test_extension_enable_failure_restores_previous_backend(self):
        self.run_setup(expected=None, failure="extension-enable")
        self.assert_old_backend()
        self.assertTrue(self.state()["active"])

    def test_real_enable_helper_handles_first_login_and_reinstall(self):
        # GSettings' keyfile backend isolates these real GJS calls from the desktop.
        env = dict(os.environ, HOME=str(self.home),
                   XDG_CONFIG_HOME=str(self.root / "isolated-settings"),
                   GSETTINGS_BACKEND="keyfile")
        uuid = json.loads((self.project / "metadata.json").read_text())["uuid"]

        def settings(action, key, *value):
            return subprocess.check_output(["gsettings", action, "org.gnome.shell", key, *value],
                                           env=env, text=True, timeout=5).strip()

        def values(key):
            return ast.literal_eval(settings("get", key).removeprefix("@as "))

        def enable():
            subprocess.run(["bash", "-euc", 'source "$1"; enable_extension', "test",
                            str(self.project / "scripts/common.sh")],
                           env=env, check=True, capture_output=True, text=True, timeout=5)

        # No running GNOME Shell needs to know this UUID yet.
        enable()
        self.assertEqual(values("enabled-extensions"), [uuid])
        self.assertEqual(values("disabled-extensions"), [])
        settings("set", "enabled-extensions", "['other@example.org']")
        settings("set", "disabled-extensions", repr([uuid, "disabled@example.org"]))
        settings("set", "disable-user-extensions", "true")
        enable()
        enable()
        self.assertEqual(values("enabled-extensions"), ["other@example.org", uuid])
        self.assertEqual(values("disabled-extensions"), ["disabled@example.org"])
        self.assertEqual(settings("get", "disable-user-extensions"), "true")

    def test_preparation_failures_leave_existing_installation_running(self):
        for failure in ("build", "tests", "pack", "bad-package", "stage", "incomplete-stage", "extension-install"):
            with self.subTest(failure=failure):
                (self.root / "events.jsonl").unlink(missing_ok=True)
                self.run_setup(expected=None, failure=failure)
                self.assert_old_backend()
                self.assertTrue(self.state()["active"])
                self.assertNotIn(("systemctl", "stop"), self.commands())

    def test_install_stops_old_binary_only_after_build_test_pack_and_stage(self):
        self.run_setup()
        events = self.events()
        stop = self.commands().index(("systemctl", "stop"))
        required_steps = (("cargo", "build"), ("cargo", "test"),
                          ("gnome-extensions", "pack"), ("install", "-D"),
                          ("gnome-extensions", "install"))
        for tool, argument in required_steps:
            self.assertIn((tool, argument), self.commands()[:stop])
        self.assertEqual(events[stop]["binary"], "old backend")
        enabled = events[self.commands().index(("systemctl", "enable"))]
        self.assertEqual(enabled["binary"], "new backend")
        self.assertEqual((self.prefix / "bin/airpods-gnome").read_text(), "new backend")
        self.assertEqual((self.prefix / "share/systemd/user/airpods-gnome.service").read_text(), "new unit")
        staged_installs = [event for event in events[:stop] if event["tool"] == "install"]
        self.assertEqual(len(staged_installs), 3)
        self.assertTrue(self.state()["active"])

    def test_failed_replacement_restores_all_changed_files(self):
        self.run_setup(expected=None, failure="replace")
        self.assert_old_backend()
        self.assertTrue(self.state()["active"])

    def test_failed_start_keeps_previously_disabled_service_disabled(self):
        self.write_state(active=False, enabled=False, exists=True)
        self.run_setup(expected=None, failure="service-start")
        self.assert_old_backend()
        state = self.state()
        self.assertFalse(state["active"])
        self.assertFalse(state["enabled"])

    def test_successful_first_install_enables_and_starts_backend(self):
        shutil.rmtree(self.home)
        self.write_state(active=False, enabled=False, exists=False)
        self.run_setup()
        self.assertEqual((self.prefix / "bin/airpods-gnome").read_text(), "new backend")
        self.assertEqual(self.state(), {"active": True, "enabled": True, "exists": True})
        self.assertNotIn(("systemctl", "stop"), self.commands())
        self.assertEqual({str(path.relative_to(self.prefix)) for path in self.prefix.rglob('*') if path.is_file()},
                         {"bin/airpods-gnome", "bin/airpods-gnome-ctl",
                          "share/systemd/user/airpods-gnome.service"})
        self.assertIn(("gjs", "-c"), self.commands())

    def test_failed_service_start_restores_existing_backend(self):
        result = self.run_setup(expected=None, failure="service-start")
        self.assertIn("restoring previous files", result.stderr)
        self.assert_old_backend()
        self.assertEqual(self.state(), {"active": True, "enabled": True, "exists": True})

    def test_failed_stop_restores_a_previously_running_service(self):
        self.run_setup(expected=None, failure="service-stop")
        self.assert_old_backend()
        self.assertTrue(self.state()["active"])

    def test_failed_first_install_removes_new_files_and_disables_service(self):
        for failure in ("replace", "service-start", "extension-enable"):
            with self.subTest(failure=failure):
                shutil.rmtree(self.home)
                self.write_state(active=False, enabled=False, exists=False)
                self.run_setup(expected=None, failure=failure)
                for relative in ("bin/airpods-gnome", "bin/airpods-gnome-ctl",
                                 "share/systemd/user/airpods-gnome.service"):
                    self.assertFalse((self.prefix / relative).exists())
                self.assertEqual(self.state(), {"active": False, "enabled": False, "exists": False})

    def test_failed_recovery_retains_previous_files_for_manual_repair(self):
        result = self.run_setup(expected=None, failure="service-recovery")
        self.assertIn("Recovery could not finish", result.stderr)
        backups = list(self.root.glob("airpods-gnome-install.*/previous/bin/airpods-gnome"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_text(), "old backend")

    def test_missing_session_fails_before_build_or_install(self):
        result = self.run_setup(expected=None, failure="session")
        self.assertIn("Cannot reach the user session", result.stderr)
        self.assert_old_backend()
        self.assertEqual(len(self.events()), 1)

    def test_bad_arguments_and_parallelism_are_rejected(self):
        for args in (("--unknown",), ("--build-only", "unexpected")):
            with self.subTest(args=args):
                self.run_setup(*args, expected=2)
        self.env["CARGO_BUILD_JOBS"] = "0"
        result = self.run_setup("--build-only", expected=2)
        self.assertIn("positive integer", result.stderr)
        self.assert_old_backend()

    def test_missing_lockfile_is_rejected_before_build(self):
        (self.project / "daemon/Cargo.lock").unlink()
        result = self.run_setup("--build-only", expected=None)
        self.assertIn("Bundled backend source is missing", result.stderr)
        self.assertFalse(any(event["tool"] == "cargo" for event in self.events()))


if __name__ == "__main__":
    unittest.main(verbosity=2)
