#!/usr/bin/env python3
"""Exercise setup failures and replacement ordering without touching the desktop."""

from pathlib import Path
import hashlib
import json
import os
import re
import shutil
import subprocess
import tempfile
import unittest


PROJECT = Path(__file__).resolve().parent.parent
LEGACY_DESKTOP_PATH = "share/applications/me.kavishdevar.librepods.desktop"
LEGACY_DESKTOP = b"""[Desktop Entry]
Version=1.0
Type=Application
Name=OpenPods
Comment=AirPods controller for Linux (OpenPods, fork of LibrePods)
Exec=librepods
Icon=librepods
Terminal=false
Categories=Audio;AudioVideo;Utility;Qt;
"""
MOCK_TOOL = r'''#!/usr/bin/env python3
from pathlib import Path
import json
import os
import shutil
import sys
from zipfile import ZipFile

name = Path(sys.argv[0]).name
args = sys.argv[1:]
root = Path(os.environ["SETUP_TEST_ROOT"])
project = root / "project with spaces"
state_path = root / "service.json"
state = json.loads(state_path.read_text())
event = {"tool": name, "args": args}
if name == "systemctl":
    service_name = args[-1] if args[-1].endswith(".service") else "airpods-gnome.service"
    binary = root / "desktop home/.local/bin" / service_name.removesuffix(".service")
    event["binary"] = binary.read_text() if binary.exists() else None
    event["legacy_desktop_exists"] = (root / "desktop home/.local/share/applications/me.kavishdevar.librepods.desktop").exists()
with (root / "events.jsonl").open("a") as events:
    events.write(json.dumps(event) + "\n")

failure = os.environ.get("SETUP_TEST_FAIL", "")
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
elif name == "systemctl":
    verb = args[1]
    service_state = state.get("legacy", {"active": False, "enabled": False, "exists": False}) if service_name == "librepods.service" else state
    if verb == "show-environment" and failure == "session":
        sys.exit(18)
    elif verb == "is-active":
        sys.exit(0 if service_state["active"] else 3)
    elif verb == "is-enabled":
        sys.exit(0 if service_state["enabled"] else 1)
    elif verb == "show":
        if "--property=FragmentPath" in args:
            default_path = str(root / "desktop home/.local/share/systemd/user" / service_name)
            print(service_state.get("fragment_path", default_path) if service_state["exists"] else "")
        elif "--property=DropInPaths" in args:
            print(service_state.get("drop_ins", ""))
        else:
            print("loaded" if service_state["exists"] else "not-found")
    elif verb == "stop":
        service_state["active"] = False
        marker = root / "stop-failed"
        if failure == "service-stop" and not marker.exists():
            marker.touch()
            state_path.write_text(json.dumps(state))
            sys.exit(21)
    elif verb == "enable":
        service_state["enabled"] = True
        service_state["exists"] = True
        if service_name == "airpods-gnome.service" and failure in ("service-start", "service-recovery"):
            state_path.write_text(json.dumps(state))
            sys.exit(19)
        if "--now" in args:
            service_state["active"] = True
    elif verb == "disable":
        service_state["enabled"] = False
    elif verb == "start":
        if failure == "service-recovery":
            sys.exit(20)
        service_state["active"] = True
    elif verb == "daemon-reload":
        unit_directory = root / "desktop home/.local/share/systemd/user"
        state["exists"] = (unit_directory / "airpods-gnome.service").exists()
        if "legacy" in state:
            state["legacy"]["exists"] = (unit_directory / "librepods.service").exists()
    state_path.write_text(json.dumps(state))
'''


class SetupTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="airpods-setup-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.project = self.root / "project with spaces"
        self.project.mkdir()
        for relative in ("setup", "metadata.json", "extension.js", "model.js", "backend.js", "stylesheet.css", "LICENSE",
                         "scripts/common.sh", "scripts/build-backend.sh", "scripts/pack.sh", "scripts/check-package.py"):
            target = self.project / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(PROJECT / relative, target)
        shutil.copytree(PROJECT / "icons", self.project / "icons")
        (self.project / "daemon").mkdir()
        (self.project / "daemon/Cargo.toml").write_text("# mock backend\n")
        (self.project / "daemon/Cargo.lock").write_text("# mock lockfile\n")
        (self.project / "daemon/airpods-gnome.service").write_text("new unit")
        self.desktop_home = self.root / "desktop home"
        self.prefix = self.desktop_home / ".local"
        for relative in ("bin/airpods-gnome", "bin/airpods-gnome-ctl", "share/systemd/user/airpods-gnome.service"):
            target = self.prefix / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text("old backend" if relative.startswith("bin/") else "old unit")
            target.chmod(0o755 if relative.startswith("bin/") else 0o644)
        self.config = self.desktop_home / ".config/AirPodsTrayApp/settings.conf"
        self.config.parent.mkdir(parents=True)
        self.config.write_text("earDetection=1\n")
        self.state_path = self.root / "service.json"
        self.state_path.write_text(json.dumps({"active": True, "enabled": True, "exists": True}))
        mock_bin = self.root / "mock-bin"
        mock_bin.mkdir()
        for tool in ("cargo", "rustc", "gnome-extensions", "systemctl", "install", "wpctl", "pw-dump"):
            executable = mock_bin / tool
            executable.write_text(MOCK_TOOL)
            executable.chmod(0o755)
        self.env = dict(os.environ, HOME=str(self.desktop_home),
                        PATH=str(mock_bin) + os.pathsep + os.environ["PATH"],
                        SETUP_TEST_ROOT=str(self.root), TMPDIR=str(self.root))
        self.env.pop("CARGO_BUILD_JOBS", None)

    def run_setup(self, *args, failure=""):
        self.env["SETUP_TEST_FAIL"] = failure
        result = subprocess.run([str(self.project / "setup"), *args], cwd=self.root,
                                env=self.env, capture_output=True, text=True, timeout=30)
        self.assertEqual(self.config.read_text(), "earDetection=1\n")
        return result

    def events(self):
        path = self.root / "events.jsonl"
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def assert_old_backend(self):
        self.assertEqual((self.prefix / "bin/airpods-gnome").read_text(), "old backend")
        self.assertEqual((self.prefix / "bin/airpods-gnome-ctl").read_text(), "old backend")
        self.assertEqual((self.prefix / "share/systemd/user/airpods-gnome.service").read_text(), "old unit")

    def make_legacy_install(self):
        for old, new in (("bin/librepods", "bin/airpods-gnome"),
                         ("bin/librepods-ctl", "bin/airpods-gnome-ctl"),
                         ("share/systemd/user/librepods.service", "share/systemd/user/airpods-gnome.service")):
            (self.prefix / new).rename(self.prefix / old)
        legacy_state = json.loads(self.state_path.read_text())
        self.state_path.write_text(json.dumps({"active": False, "enabled": False, "exists": False,
                                               "legacy": legacy_state}))
        # Only the isolated copy trusts our tiny fixtures; production pins remain
        # the exact previously installed ELF binaries and service files.
        setup = self.project / "setup"
        contents = setup.read_text()
        for relative in ("bin/librepods", "bin/librepods-ctl", "share/systemd/user/librepods.service"):
            digest = hashlib.sha256((self.prefix / relative).read_bytes()).hexdigest()
            pattern = r"(\[" + re.escape(relative) + r"\]=)'[^']*'"
            contents, count = re.subn(pattern, lambda match: match[1] + "'" + digest + "'", contents)
            self.assertEqual(count, 1)
        setup.write_text(contents)

    def assert_legacy_backend(self):
        self.assertEqual((self.prefix / "bin/librepods").read_text(), "old backend")
        self.assertEqual((self.prefix / "bin/librepods-ctl").read_text(), "old backend")
        self.assertFalse((self.prefix / "bin/librepods-ctl").is_symlink())
        self.assertEqual((self.prefix / "share/systemd/user/librepods.service").read_text(), "old unit")

    def test_build_only_leaves_installation_and_service_untouched(self):
        result = self.run_setup("--build-only")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assert_old_backend()
        self.assertFalse(any(event["tool"] == "systemctl" for event in self.events()))
        self.assertFalse(any(event["tool"] == "gnome-extensions" and event["args"][0] == "install"
                             for event in self.events()))
        self.assertTrue(list((self.project / "dist").glob("*.zip")))

    def test_preparation_failures_leave_existing_installation_running(self):
        for failure in ("build", "tests", "pack", "bad-package", "stage", "incomplete-stage", "extension-install"):
            with self.subTest(failure=failure):
                (self.root / "events.jsonl").unlink(missing_ok=True)
                result = self.run_setup(failure=failure)
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assert_old_backend()
                self.assertTrue(json.loads(self.state_path.read_text())["active"])
                self.assertFalse(any(event["tool"] == "systemctl" and event["args"][1] == "stop"
                                     for event in self.events()), result.stdout + result.stderr)

    def test_install_stops_old_binary_only_after_build_test_pack_and_stage(self):
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        events = self.events()
        stop = next(i for i, event in enumerate(events)
                    if event["tool"] == "systemctl" and event["args"][1] == "stop")
        required_steps = (("cargo", "build"), ("cargo", "test"),
                          ("gnome-extensions", "pack"), ("install", "-D"),
                          ("gnome-extensions", "install"))
        for tool, argument in required_steps:
            self.assertTrue(any(event["tool"] == tool and argument in event["args"]
                                for event in events[:stop]), (tool, argument, events))
        self.assertEqual(events[stop]["binary"], "old backend")
        enabled = next(event for event in events if event["tool"] == "systemctl" and event["args"][1] == "enable")
        self.assertEqual(enabled["binary"], "new backend")
        self.assertEqual((self.prefix / "bin/airpods-gnome").read_text(), "new backend")
        self.assertEqual((self.prefix / "share/systemd/user/airpods-gnome.service").read_text(), "new unit")
        staged_installs = [event for event in events[:stop] if event["tool"] == "install"]
        self.assertEqual(len(staged_installs), 3)
        self.assertTrue(json.loads(self.state_path.read_text())["active"])

    def test_failed_replacement_restores_all_changed_files(self):
        result = self.run_setup(failure="replace")
        self.assertNotEqual(result.returncode, 0)
        self.assert_old_backend()
        self.assertTrue(json.loads(self.state_path.read_text())["active"])

    def test_failed_start_keeps_previously_disabled_service_disabled(self):
        self.state_path.write_text(json.dumps({"active": False, "enabled": False, "exists": True}))
        result = self.run_setup(failure="service-start")
        self.assertNotEqual(result.returncode, 0)
        self.assert_old_backend()
        state = json.loads(self.state_path.read_text())
        self.assertFalse(state["active"])
        self.assertFalse(state["enabled"])

    def test_successful_first_install_enables_and_starts_backend(self):
        shutil.rmtree(self.prefix)
        self.state_path.write_text(json.dumps({"active": False, "enabled": False, "exists": False}))
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((self.prefix / "bin/airpods-gnome").read_text(), "new backend")
        self.assertEqual(json.loads(self.state_path.read_text()), {"active": True, "enabled": True, "exists": True})
        self.assertFalse(any(event["tool"] == "systemctl" and event["args"][1] == "stop"
                             for event in self.events()))

    def test_failed_service_start_restores_existing_backend(self):
        result = self.run_setup(failure="service-start")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("restoring previous files", result.stderr)
        self.assert_old_backend()
        self.assertEqual(json.loads(self.state_path.read_text()), {"active": True, "enabled": True, "exists": True})

    def test_failed_stop_restores_a_previously_running_service(self):
        result = self.run_setup(failure="service-stop")
        self.assertNotEqual(result.returncode, 0)
        self.assert_old_backend()
        self.assertTrue(json.loads(self.state_path.read_text())["active"])

    def test_failed_first_install_removes_new_files_and_disables_service(self):
        shutil.rmtree(self.prefix)
        self.state_path.write_text(json.dumps({"active": False, "enabled": False, "exists": False}))
        result = self.run_setup(failure="service-start")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.prefix / "bin/airpods-gnome").exists())
        self.assertFalse((self.prefix / "share/systemd/user/airpods-gnome.service").exists())
        state = json.loads(self.state_path.read_text())
        self.assertFalse(state["active"])
        self.assertFalse(state["enabled"])

    def test_failed_recovery_retains_previous_files_for_manual_repair(self):
        result = self.run_setup(failure="service-recovery")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Recovery could not finish", result.stderr)
        backups = list(self.root.glob("airpods-gnome-install.*/previous/bin/airpods-gnome"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_text(), "old backend")

    def test_missing_session_fails_before_build_or_install(self):
        result = self.run_setup(failure="session")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Cannot reach the user session", result.stderr)
        self.assert_old_backend()
        self.assertEqual(len(self.events()), 1)

    def test_bad_arguments_and_parallelism_are_rejected(self):
        for args in (("--unknown",), ("--build-only", "unexpected")):
            with self.subTest(args=args):
                result = self.run_setup(*args)
                self.assertEqual(result.returncode, 2, result.stderr)
        self.env["CARGO_BUILD_JOBS"] = "0"
        result = self.run_setup("--build-only")
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn("positive integer", result.stderr)
        self.assert_old_backend()

    def test_missing_lockfile_is_rejected_before_build(self):
        (self.project / "daemon/Cargo.lock").unlink()
        result = self.run_setup("--build-only")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Bundled backend source is missing", result.stderr)
        self.assertFalse(any(event["tool"] == "cargo" for event in self.events()))

    def test_legacy_service_migrates_after_preparation_and_control_alias_survives_updates(self):
        self.make_legacy_install()
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse((self.prefix / "bin/librepods").exists())
        self.assertFalse((self.prefix / "share/systemd/user/librepods.service").exists())
        alias = self.prefix / "bin/librepods-ctl"
        self.assertTrue(alias.is_symlink())
        self.assertEqual(os.readlink(alias), "airpods-gnome-ctl")
        self.assertEqual(alias.read_text(), "new backend")
        state = json.loads(self.state_path.read_text())
        self.assertEqual(state["legacy"], {"active": False, "enabled": False, "exists": False})
        self.assertTrue(state["active"] and state["enabled"])
        events = self.events()
        stop = next(i for i, event in enumerate(events) if event["tool"] == "systemctl" and event["args"][1] == "stop")
        self.assertEqual(events[stop]["args"][-1], "librepods.service")
        self.assertEqual(events[stop]["binary"], "old backend")
        self.assertTrue(any(event["tool"] == "gnome-extensions" and event["args"][0] == "install" for event in events[:stop]))
        disable = next(i for i, event in enumerate(events) if event["tool"] == "systemctl" and event["args"][1] == "disable")
        start = next(i for i, event in enumerate(events) if event["tool"] == "systemctl" and event["args"][1] == "enable")
        self.assertLess(disable, start)
        self.assertEqual(events[start]["args"][-1], "airpods-gnome.service")
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(alias.is_symlink())
        self.assertEqual(alias.read_text(), "new backend")

    def test_current_install_does_not_create_a_legacy_alias(self):
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        alias = self.prefix / "bin/librepods-ctl"
        self.assertFalse(alias.exists() or alias.is_symlink())

    def test_retired_migration_alias_is_not_recreated_by_updates(self):
        self.make_legacy_install()
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        alias = self.prefix / "bin/librepods-ctl"
        self.assertTrue(alias.is_symlink())
        alias.unlink()
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(alias.exists() or alias.is_symlink())

    def test_legacy_migration_rolls_back_names_alias_and_service_state(self):
        self.make_legacy_install()
        for failure in ("service-stop", "replace", "service-start"):
            with self.subTest(failure=failure):
                result = self.run_setup(failure=failure)
                self.assertNotEqual(result.returncode, 0)
                self.assert_legacy_backend()
                state = json.loads(self.state_path.read_text())
                self.assertEqual(state["legacy"], {"active": True, "enabled": True, "exists": True})
                self.assertFalse(state["active"] or state["enabled"])
                self.assertFalse((self.prefix / "bin/airpods-gnome").exists())
                self.assertFalse((self.prefix / "share/systemd/user/airpods-gnome.service").exists())

    def test_disabled_legacy_service_stays_disabled_when_migration_fails(self):
        self.make_legacy_install()
        state = json.loads(self.state_path.read_text())
        state["legacy"].update(active=False, enabled=False)
        self.state_path.write_text(json.dumps(state))
        result = self.run_setup(failure="service-start")
        self.assertNotEqual(result.returncode, 0)
        self.assert_legacy_backend()
        self.assertEqual(json.loads(self.state_path.read_text())["legacy"],
                         {"active": False, "enabled": False, "exists": True})

    def test_custom_legacy_daemon_is_not_stopped_or_overwritten(self):
        self.make_legacy_install()
        daemon = self.prefix / "bin/librepods"
        daemon.write_text("custom daemon")
        result = self.run_setup()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("customized or unrelated", result.stderr)
        self.assertEqual(daemon.read_text(), "custom daemon")
        self.assertTrue(json.loads(self.state_path.read_text())["legacy"]["active"])
        self.assertFalse(any(event["tool"] == "systemctl" and event["args"][1] in ("stop", "disable", "enable")
                             for event in self.events()))

    def test_custom_legacy_control_command_is_preserved_during_migration(self):
        self.make_legacy_install()
        control = self.prefix / "bin/librepods-ctl"
        control.write_text("custom control script")
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(control.is_symlink())
        self.assertEqual(control.read_text(), "custom control script")
        self.assertTrue(json.loads(self.state_path.read_text())["active"])

    def test_custom_legacy_service_override_is_preserved_when_active_or_dormant(self):
        self.make_legacy_install()
        state = json.loads(self.state_path.read_text())
        state["legacy"]["drop_ins"] = str(self.desktop_home / ".config/systemd/user/librepods.service.d/custom.conf")
        for active, enabled in ((True, True), (False, False)):
            with self.subTest(active=active, enabled=enabled):
                state["legacy"].update(active=active, enabled=enabled)
                self.state_path.write_text(json.dumps(state))
                result = self.run_setup()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("customized or unrelated", result.stderr)
                self.assert_legacy_backend()
                self.assertEqual(json.loads(self.state_path.read_text()), state)
                self.assertFalse(any(event["tool"] == "systemctl" and event["args"][1] in ("stop", "disable", "enable")
                                     for event in self.events()))
                self.assertFalse(any(event["tool"] == "gnome-extensions" and event["args"][0] == "install"
                                     for event in self.events()))

    def test_generic_service_drop_ins_allow_legacy_migration_from_any_root(self):
        self.make_legacy_install()
        state = json.loads(self.state_path.read_text())
        state["legacy"]["drop_ins"] = " ".join((
            "/usr/lib/systemd/user/service.d/10-timeout-abort.conf",
            "/etc/systemd/user/service.d/20-defaults.conf",
            "/custom/user-config/systemd/user/service.d/30-limits.conf",
        ))
        self.state_path.write_text(json.dumps(state))
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        state = json.loads(self.state_path.read_text())
        self.assertTrue(state["active"] and state["enabled"])
        self.assertFalse(state["legacy"]["active"] or state["legacy"]["enabled"])
        self.assertTrue((self.prefix / "bin/librepods-ctl").is_symlink())

    def test_generic_drop_ins_do_not_hide_a_custom_legacy_service_drop_in(self):
        self.make_legacy_install()
        state = json.loads(self.state_path.read_text())
        global_drop_in = "/usr/lib/systemd/user/service.d/10-timeout-abort.conf"
        custom_drop_in = "/custom/systemd/user/librepods.service.d/custom.conf"
        for drop_ins in ((global_drop_in, custom_drop_in), (custom_drop_in, global_drop_in)):
            with self.subTest(drop_ins=drop_ins):
                state["legacy"]["drop_ins"] = " ".join(drop_ins)
                self.state_path.write_text(json.dumps(state))
                result = self.run_setup()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("customized or unrelated", result.stderr)
                self.assert_legacy_backend()
                self.assertEqual(json.loads(self.state_path.read_text()), state)
                self.assertFalse(any(event["tool"] == "systemctl" and event["args"][1] in ("stop", "disable", "enable")
                                     for event in self.events()))

    def test_matching_retired_launcher_is_removed_and_restored_on_failure(self):
        desktop = self.prefix / LEGACY_DESKTOP_PATH
        desktop.parent.mkdir(parents=True)
        for failure in ("service-start", ""):
            with self.subTest(failure=failure):
                desktop.write_bytes(LEGACY_DESKTOP)
                desktop.chmod(0o640)
                (self.root / "events.jsonl").unlink(missing_ok=True)
                result = self.run_setup(failure=failure)
                enabled = next(event for event in self.events()
                               if event["tool"] == "systemctl" and event["args"][1] == "enable")
                self.assertFalse(enabled["legacy_desktop_exists"], "obsolete launcher remains after replacement")
                if failure:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(desktop.read_bytes(), LEGACY_DESKTOP)
                    self.assertEqual(desktop.stat().st_mode & 0o777, 0o640)
                    self.assert_old_backend()
                else:
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertFalse(desktop.exists())

    def test_customized_retired_files_and_symlinks_are_preserved(self):
        desktop = self.prefix / LEGACY_DESKTOP_PATH
        icon = self.prefix / "share/icons/hicolor/scalable/apps/librepods.svg"
        translation = self.prefix / "share/openpods/translations/openpods_tr.qm"
        for path in (desktop, icon, translation):
            path.parent.mkdir(parents=True, exist_ok=True)
        custom_desktop = LEGACY_DESKTOP.replace(b"Exec=librepods", b"Exec=my-custom-controller")
        desktop.write_bytes(custom_desktop)
        icon.write_bytes(b"custom icon")
        translation.write_bytes(b"custom translation")
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(desktop.read_bytes(), custom_desktop)
        self.assertEqual(icon.read_bytes(), b"custom icon")
        self.assertEqual(translation.read_bytes(), b"custom translation")

        # Even a symlink whose destination exactly matches the retired launcher
        # belongs to the user, not this installer's original regular files.
        linked_file = self.root / "user-managed.desktop"
        linked_file.write_bytes(LEGACY_DESKTOP)
        desktop.unlink()
        desktop.symlink_to(linked_file)
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(desktop.is_symlink())
        self.assertEqual(linked_file.read_bytes(), LEGACY_DESKTOP)


if __name__ == "__main__":
    unittest.main(verbosity=2)
