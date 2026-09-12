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

root = Path(os.environ['INSTALLER_TEST_ROOT'])
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
if os.environ.get('INSTALLER_TEST_FAIL') == verb:
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
        self.project.mkdir()
        self.home = self.root / 'desktop home'
        self.state_path = self.root / 'state.json'
        self.env = dict(os.environ, HOME=str(self.home), TMPDIR=str(self.root),
                        XDG_CONFIG_HOME=str(self.home / '.config'),
                        XDG_DATA_HOME=str(self.home / 'custom data'),
                        XDG_CACHE_HOME=str(self.home / '.cache'),
                        INSTALLER_TEST_ROOT=str(self.root))
        for relative in ('uninstall', 'scripts/common.sh'):
            target = self.project / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(PROJECT / relative, target)
        (self.project / 'metadata.json').write_text(json.dumps({'uuid': 'test@example.org'}))
        self.unit = self.home / '.local/share/systemd/user/airpods-gnome.service'
        self.binary = self.home / '.local/bin/airpods-gnome'
        self.client = self.home / '.local/bin/airpods-gnome-ctl'
        for path in (self.unit, self.binary, self.client):
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('installed file')
        self.link = self.home / '.config/systemd/user/graphical-session.target.wants/airpods-gnome.service'
        self.link.parent.mkdir(parents=True)
        self.link.symlink_to(self.unit)
        self.data = Path(self.env['XDG_DATA_HOME'])
        self.extension = self.data / 'gnome-shell/extensions/test@example.org'
        self.extension.mkdir(parents=True)
        (self.extension / 'metadata.json').write_text('{}')
        self.config = self.home / '.config/AirPodsTrayApp/rust-settings.json'
        self.config.parent.mkdir(parents=True)
        self.config.write_text('saved settings')
        self.write_state(loaded=True, active=True, enabled=True,
                         extension_loaded=True, extension_enabled=True)
        commands = self.root / 'mock-bin'
        commands.mkdir()
        for name in ('systemctl', 'gnome-extensions'):
            tool = commands / name
            tool.write_text(MOCK_TOOL)
            tool.chmod(0o755)
        self.env['PATH'] = str(commands) + os.pathsep + os.environ['PATH']

    def run_uninstall(self, *args, failure='', expected=0):
        result = subprocess.run([str(self.project / 'uninstall'), *args], cwd=self.root,
                                env=self.env | {'INSTALLER_TEST_FAIL': failure},
                                capture_output=True, text=True, timeout=30)
        if expected is None:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        else:
            self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        self.assertEqual(self.config.read_text(), 'saved settings')
        self.assertTrue((self.project / 'uninstall').is_file())
        return result

    def state(self):
        return json.loads(self.state_path.read_text()) if self.state_path.exists() else {}

    def write_state(self, **values):
        self.state_path.write_text(json.dumps(self.state() | values))

    def events(self):
        log = self.root / 'events.jsonl'
        return [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []

    def assert_installed(self):
        for path in (self.binary, self.client, self.unit):
            self.assertTrue(path.exists(), path)

    def test_complete_uninstall_and_repeated_run(self):
        for _ in range(2):
            self.run_uninstall()
            for path in (self.binary, self.client, self.unit, self.link, self.extension):
                self.assertFalse(path.exists() or path.is_symlink(), path)
        state = self.state()
        self.assertFalse(state['active'] or state['enabled'] or state['loaded'])
        events = self.events()
        self.assertEqual(events.count(['systemctl', 'stop']), 1)
        self.assertLess(events.index(['systemctl', 'stop']), events.index(['systemctl', 'disable']))

    def test_help_and_invalid_arguments_do_not_touch_installation(self):
        for args, expected in ((('--help',), 0), (('--purge',), 2), (('', 'extra'), 2)):
            self.run_uninstall(*args, expected=expected)
            self.assert_installed()
        self.assertEqual(self.events(), [])

    def test_missing_session_keeps_installation(self):
        self.run_uninstall(expected=None, failure='show-environment')
        self.assert_installed()
        self.assertTrue(self.extension.exists())

    def test_foreign_service_keeps_installation(self):
        self.write_state(fragment='/different/airpods-gnome.service')
        result = self.run_uninstall(expected=None)
        self.assertIn('different installation', result.stderr)
        self.assert_installed()
        self.assertTrue(self.extension.exists())
        self.assertNotIn(['systemctl', 'stop'], self.events())

    def test_stop_or_disable_failure_preserves_backend_files(self):
        for failure in ('stop', 'disable'):
            with self.subTest(failure=failure):
                self.run_uninstall(expected=None, failure=failure)
                self.assert_installed()

    def test_extension_failure_does_not_stop_backend(self):
        self.run_uninstall(expected=None, failure='uninstall')
        self.assert_installed()
        self.assertNotIn(['systemctl', 'stop'], self.events())

    def test_extension_on_disk_before_first_login_is_removed(self):
        self.write_state(extension_loaded=False, extension_enabled=False)
        self.run_uninstall()
        self.assertFalse(self.extension.exists())
        self.assertNotIn(['gnome-extensions', 'disable'], self.events())

    def test_symlinked_extension_preserves_source_checkout(self):
        other = self.root / 'extension source'
        self.extension.rename(other)
        self.extension.symlink_to(other, target_is_directory=True)
        self.run_uninstall()
        self.assertFalse(self.extension.is_symlink())
        self.assertTrue((other / 'metadata.json').exists())
        self.assertNotIn(['gnome-extensions', 'uninstall'], self.events())

    def test_missing_unit_cleans_dangling_startup_link(self):
        self.unit.unlink()
        self.write_state(loaded=False, active=False, enabled=False)
        self.run_uninstall()
        self.assertFalse(self.link.is_symlink())
        self.assertNotIn(['systemctl', 'stop'], self.events())

    def test_setup_lock_prevents_concurrent_uninstall(self):
        (self.project / 'build').mkdir()
        with (self.project / 'build/setup.lock').open('w') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            result = self.run_uninstall(expected=None)
        self.assertIn('Another setup or uninstall', result.stderr)
        self.assert_installed()
        self.assertTrue(self.extension.exists())

    def test_directory_in_place_of_binary_aborts_before_removal(self):
        self.binary.unlink()
        self.binary.mkdir()
        self.run_uninstall(expected=None)
        self.assertTrue(self.binary.is_dir())
        self.assertTrue(self.extension.exists())
        self.assertTrue(self.unit.exists())
        self.assertNotIn(['systemctl', 'stop'], self.events())


if __name__ == '__main__':
    unittest.main()
