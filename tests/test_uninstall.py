#!/usr/bin/env python3
"""Exercise uninstall against an isolated home and mocked desktop services."""

from pathlib import Path
import fcntl
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

root = Path(os.environ['UNINSTALL_TEST_ROOT'])
state_path = root / 'state.json'
state = json.loads(state_path.read_text())
home = Path(os.environ['HOME'])
unit = home / '.local/share/systemd/user/airpods-gnome.service'
link = home / '.config/systemd/user/graphical-session.target.wants/airpods-gnome.service'
extension = Path(os.environ['XDG_DATA_HOME']) / 'gnome-shell/extensions/test@example.org'
tool = Path(sys.argv[0]).name
args = sys.argv[1:]
verb = args[1] if tool == 'systemctl' else args[0]
with (root / 'events.jsonl').open('a') as output:
    output.write(json.dumps([tool, verb]) + '\n')
if os.environ.get('UNINSTALL_TEST_FAIL') == verb:
    sys.exit(12)
if tool == 'systemctl':
    assert args[0] == '--user'
    if verb == 'show':
        if '--property=FragmentPath' in args:
            print(state.get('fragment', str(unit)))
        else:
            print('loaded' if state['loaded'] else 'not-found')
    elif verb == 'stop':
        assert (home / '.local/bin/airpods-gnome').exists()
        state['active'] = False
    elif verb == 'disable':
        assert not state['active']
        state['enabled'] = False
        link.unlink(missing_ok=True)
    elif verb == 'daemon-reload':
        state['loaded'] = unit.exists()
    else:
        assert verb == 'show-environment'
else:
    assert tool == 'gnome-extensions'
    if verb == 'list':
        assert args == ['list', '--user']
        if state['extension_loaded']:
            print('test@example.org')
    elif verb == 'disable':
        assert state['extension_loaded']
        state['extension_enabled'] = False
    elif verb == 'uninstall':
        assert not state['extension_enabled']
        shutil.rmtree(extension)
        state['extension_loaded'] = False
    else:
        raise AssertionError(args)
state_path.write_text(json.dumps(state))
'''


class UninstallTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='airpods-uninstall-test-')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.project = self.root / 'project with spaces'
        (self.project / 'scripts').mkdir(parents=True)
        for name in ('uninstall', 'scripts/common.sh'):
            shutil.copy2(PROJECT / name, self.project / name)
        (self.project / 'metadata.json').write_text(json.dumps({'uuid': 'test@example.org'}))
        self.home = self.root / 'desktop home'
        self.unit = self.home / '.local/share/systemd/user/airpods-gnome.service'
        self.binary = self.home / '.local/bin/airpods-gnome'
        self.client = self.home / '.local/bin/airpods-gnome-ctl'
        for path in (self.unit, self.binary, self.client):
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('installed file')
        self.alias = self.home / '.local/bin/librepods-ctl'
        self.alias.symlink_to('airpods-gnome-ctl')
        self.link = self.home / '.config/systemd/user/graphical-session.target.wants/airpods-gnome.service'
        self.link.parent.mkdir(parents=True)
        self.link.symlink_to(self.unit)
        self.data = self.home / 'custom data'
        self.extension = self.data / 'gnome-shell/extensions/test@example.org'
        self.extension.mkdir(parents=True)
        (self.extension / 'metadata.json').write_text('{}')
        self.config = self.home / '.config/AirPodsTrayApp/rust-settings.json'
        self.config.parent.mkdir(parents=True)
        self.config.write_text('saved settings')
        self.state_path = self.root / 'state.json'
        self.write_state(loaded=True, active=True, enabled=True,
                         extension_loaded=True, extension_enabled=True)
        commands = self.root / 'mock-bin'
        commands.mkdir()
        for name in ('systemctl', 'gnome-extensions'):
            tool = commands / name
            tool.write_text(MOCK_TOOL)
            tool.chmod(0o755)
        self.env = dict(os.environ, HOME=str(self.home), XDG_DATA_HOME=str(self.data),
                        PATH=str(commands) + os.pathsep + os.environ['PATH'],
                        UNINSTALL_TEST_ROOT=str(self.root))

    def write_state(self, **values):
        current = json.loads(self.state_path.read_text()) if self.state_path.exists() else {}
        self.state_path.write_text(json.dumps(current | values))

    def events(self):
        log = self.root / 'events.jsonl'
        return [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []

    def run_uninstall(self, *args, failure=''):
        result = subprocess.run([str(self.project / 'uninstall'), *args], cwd=self.root,
                                env=self.env | {'UNINSTALL_TEST_FAIL': failure},
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(self.config.read_text(), 'saved settings')
        self.assertTrue((self.project / 'uninstall').is_file())
        return result

    def assert_installed(self):
        for path in (self.binary, self.client, self.unit):
            self.assertTrue(path.exists(), path)

    def test_complete_uninstall_and_repeated_run(self):
        for _ in range(2):
            result = self.run_uninstall()
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            for path in (self.binary, self.client, self.unit, self.alias, self.link, self.extension):
                self.assertFalse(path.exists() or path.is_symlink(), path)
        state = json.loads(self.state_path.read_text())
        self.assertFalse(state['active'] or state['enabled'] or state['loaded'])
        events = self.events()
        self.assertEqual(events.count(['systemctl', 'stop']), 1)
        self.assertLess(events.index(['systemctl', 'stop']), events.index(['systemctl', 'disable']))

    def test_help_and_invalid_arguments_do_not_touch_installation(self):
        for args, expected in ((('--help',), 0), (('--purge',), 2), (('', 'extra'), 2)):
            result = self.run_uninstall(*args)
            self.assertEqual(result.returncode, expected)
            self.assert_installed()
        self.assertEqual(self.events(), [])

    def test_missing_session_keeps_installation(self):
        self.assertNotEqual(self.run_uninstall(failure='show-environment').returncode, 0)
        self.assert_installed()
        self.assertTrue(self.extension.exists())

    def test_foreign_service_keeps_installation(self):
        self.write_state(fragment='/different/airpods-gnome.service')
        result = self.run_uninstall()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('different installation', result.stderr)
        self.assert_installed()
        self.assertTrue(self.extension.exists())
        self.assertNotIn(['systemctl', 'stop'], self.events())

    def test_stop_or_disable_failure_preserves_backend_files(self):
        for failure in ('stop', 'disable'):
            with self.subTest(failure=failure):
                result = self.run_uninstall(failure=failure)
                self.assertNotEqual(result.returncode, 0)
                self.assert_installed()
                self.assertTrue(self.alias.is_symlink())

    def test_extension_failure_does_not_stop_backend(self):
        self.assertNotEqual(self.run_uninstall(failure='uninstall').returncode, 0)
        self.assert_installed()
        self.assertNotIn(['systemctl', 'stop'], self.events())

    def test_unrelated_legacy_client_is_preserved(self):
        self.alias.unlink()
        self.alias.write_text('separate legacy client')
        self.assertEqual(self.run_uninstall().returncode, 0)
        self.assertEqual(self.alias.read_text(), 'separate legacy client')

    def test_unrelated_client_symlink_is_preserved(self):
        self.alias.unlink()
        other = self.root / 'other client'
        other.write_text('keep')
        self.alias.symlink_to(other)
        self.assertEqual(self.run_uninstall().returncode, 0)
        self.assertTrue(self.alias.is_symlink())
        self.assertEqual(other.read_text(), 'keep')

    def test_extension_on_disk_before_first_login_is_removed(self):
        self.write_state(extension_loaded=False, extension_enabled=False)
        result = self.run_uninstall()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.extension.exists())
        self.assertNotIn(['gnome-extensions', 'disable'], self.events())

    def test_symlinked_extension_preserves_source_checkout(self):
        other = self.root / 'extension source'
        self.extension.rename(other)
        self.extension.symlink_to(other, target_is_directory=True)
        self.assertEqual(self.run_uninstall().returncode, 0)
        self.assertFalse(self.extension.is_symlink())
        self.assertTrue((other / 'metadata.json').exists())
        self.assertNotIn(['gnome-extensions', 'uninstall'], self.events())

    def test_missing_unit_cleans_dangling_startup_link(self):
        self.unit.unlink()
        self.write_state(loaded=False, active=False, enabled=False)
        result = self.run_uninstall()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.link.is_symlink())
        self.assertNotIn(['systemctl', 'stop'], self.events())

    def test_setup_lock_prevents_concurrent_uninstall(self):
        (self.project / 'build').mkdir()
        with (self.project / 'build/setup.lock').open('w') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            result = self.run_uninstall()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Another setup or uninstall', result.stderr)
        self.assert_installed()
        self.assertTrue(self.extension.exists())

    def test_directory_in_place_of_binary_aborts_before_removal(self):
        self.binary.unlink()
        self.binary.mkdir()
        self.assertNotEqual(self.run_uninstall().returncode, 0)
        self.assertTrue(self.binary.is_dir())
        self.assertTrue(self.extension.exists())
        self.assertTrue(self.unit.exists())
        self.assertNotIn(['systemctl', 'stop'], self.events())


if __name__ == '__main__':
    unittest.main()
