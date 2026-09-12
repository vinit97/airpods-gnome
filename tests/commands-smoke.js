import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import {Backend, findControlExecutable} from '../backend.js';

const root = GLib.dir_make_tmp('airpods-commands-test-XXXXXX');
const executable = `${root}/airpods-gnome-ctl`;
GLib.file_set_contents(executable, `#!/bin/sh
printf '%s\\n' "$1" >> "$0.log"
case "$1" in
    noise:off) echo 'Device refused command' >&2; exit 1 ;;
    adaptive:42) exec sleep 2 ;;
    adaptive:43) sleep 1.2 & wait ;;
    *) sleep 0.05 ;;
esac
`);
Gio.File.new_for_path(executable).set_attribute_uint32('unix::mode', 0o700, Gio.FileQueryInfoFlags.NONE, null);
const loop = new GLib.MainLoop(null, false);
let backend;
let failure;
const sleep = ms => new Promise(resolve => GLib.timeout_add(GLib.PRIORITY_DEFAULT, ms, () => {
    resolve(); return GLib.SOURCE_REMOVE;
}));
function assert(condition, message) { if (!condition) throw new Error(message); }
async function idle() {
    for (let i = 0; i < 100 && (backend._process || backend._queue.length); i++) await sleep(20);
    assert(!backend._process && !backend._queue.length, 'Command processing did not finish');
}
async function run() {
    const localBin = `${root}/.local/bin`;
    GLib.mkdir_with_parents(localBin, 0o700);
    const currentLocal = `${localBin}/airpods-gnome-ctl`;
    const pathExecutables = new Map();
    const findInPath = name => pathExecutables.get(name) ?? null;
    const installLocal = path => Gio.File.new_for_path(executable).copy(Gio.File.new_for_path(path),
        Gio.FileCopyFlags.NONE, null, null);
    assert(findControlExecutable(root, findInPath) === null, 'Missing backend was found');
    pathExecutables.set('airpods-gnome-ctl', executable);
    assert(findControlExecutable(root, findInPath) === executable, 'PATH lookup failed');
    installLocal(currentLocal);
    assert(findControlExecutable(root, findInPath) === currentLocal,
        'Current local backend was not preferred');
    Gio.File.new_for_path(currentLocal).set_attribute_uint32('unix::mode', 0o600, Gio.FileQueryInfoFlags.NONE, null);
    assert(findControlExecutable(root, findInPath) === executable, 'Non-executable local file masked a working backend');
    pathExecutables.delete('airpods-gnome-ctl');
    assert(findControlExecutable(root, findInPath) === null, 'Non-executable backend was accepted');

    const errors = [];
    backend = new Backend(() => {}, (message, kind, command, selection) => errors.push({message, kind, command, selection}), root,
        {ctlPath: executable, timeoutMs: 200});
    assert(!backend.command('noise:invalid') && !backend.command('adaptive:101') && !backend.command(null),
        'Invalid commands were accepted');
    assert(backend.command('noise:anc'), 'Valid command was rejected');
    backend.command('ca:on');
    backend.command('adaptive:25');
    backend.command('adaptive:75');
    await idle();
    const [, bytes] = GLib.file_get_contents(`${executable}.log`);
    assert(new TextDecoder().decode(bytes).trim() === 'noise:anc\nca:on\nadaptive:75',
        'Rapid commands were dropped, reordered or failed to coalesce');
    const earlierSelection = {};
    const replacedSelection = {};
    const latestSelection = {};
    const beforeRepeated = errors.length;
    backend.command('noise:off', earlierSelection);
    backend.command('noise:anc', replacedSelection);
    backend.command('noise:off', latestSelection);
    await idle();
    assert(errors.some(e => e.kind === 'command' && e.command === 'noise:off' && e.message.includes('Device refused')),
        'Command errors must be separate from unavailable status');
    const repeatedFailures = errors.slice(beforeRepeated).filter(e => e.kind === 'command');
    assert(repeatedFailures.length === 2 && repeatedFailures[0].selection === earlierSelection
        && repeatedFailures[1].selection === latestSelection,
        'Repeated commands or queue replacement lost their selection identities');
    const timedOutSelection = {};
    backend.command('adaptive:42', timedOutSelection);
    await idle();
    assert(errors.some(e => e.kind === 'command' && e.command === 'adaptive:42'
        && e.selection === timedOutSelection && e.message.includes('timed out')), 'Timeout lost its selection identity');
    const started = GLib.get_monotonic_time();
    backend.command('adaptive:43');
    backend.command('ear:both');
    await idle();
    assert(GLib.get_monotonic_time() - started < 800000,
        'A child holding stderr open blocked the command queue after timeout');
    assert(errors.some(e => e.kind === 'command' && e.command === 'adaptive:43' && e.message.includes('timed out')),
        'Timeout with inherited stderr was not reported');
    assert(!backend._commandCancel && !backend._timeout, 'Completed commands left cancellation or deadline state');

    backend.command('noise:anc');
    const discarded = [{}, {}, {}];
    backend.command('ca:off', discarded[0]);
    backend.command('ear:one', discarded[1]);
    backend.command('adaptive:90', discarded[2]);
    backend._ctlPath = `${root}/missing-airpods-gnome-ctl`;
    const beforeSpawnFailure = errors.length;
    await idle();
    const rejected = errors.slice(beforeSpawnFailure).filter(e => e.kind === 'command');
    assert(rejected.map(e => e.command).join(',') === 'ca:off,ear:one,adaptive:90'
        && rejected.every((e, index) => e.selection === discarded[index]),
        'Launcher failure did not report every discarded command');
    backend._ctlPath = executable;

    backend.command('noise:anc');
    backend.command('ca:off');
    const count = errors.length;
    backend.destroy();
    backend.destroy();
    assert(!backend.command('ca:on'), 'Disabled backend accepted a command');
    await sleep(100);
    assert(backend._queue.length === 0 && !backend._process && !backend._timeout && !backend._commandCancel,
        'Disable left pending commands or deadline state');
    assert(errors.length === count, 'Disable delivered a stale callback');
    print('PASS: backend lookup, command validation, coalescing, failure correlation, bounded timeouts, launcher failure, disable cleanup');
}
run().catch(error => { failure = error; }).finally(() => loop.quit());
loop.run();
backend?.destroy();
for (const path of [`${root}/librepods`, `${root}/.local/bin/airpods-gnome-ctl`,
    `${root}/.local/bin`, `${root}/.local`, executable, `${executable}.log`, root]) {
    const file = Gio.File.new_for_path(path);
    if (file.query_exists(null)) file.delete(null);
}
if (failure) throw failure;
