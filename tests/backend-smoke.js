// Run with: gjs -m tests/backend-smoke.js
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import {Backend} from '../backend.js';

const root = GLib.dir_make_tmp('airpods-gnome-test-XXXXXX');
const directory = `${root}/airpods-gnome`;
const path = `${directory}/status.json`;
const loop = new GLib.MainLoop(null, false);
let backend;
let failure;
const sleep = ms => new Promise(resolve => GLib.timeout_add(GLib.PRIORITY_DEFAULT, ms, () => {
    resolve(); return GLib.SOURCE_REMOVE;
}));
function assert(condition, message) { if (!condition) throw new Error(message); }
async function until(predicate, message) {
    for (let i = 0; i < 200 && !predicate(); i++) await sleep(10);
    assert(predicate(), message);
}
async function run() {
    const statuses = [];
    const errors = [];
    backend = new Backend(status => statuses.push(status),
        (message, kind) => errors.push({message, kind}), root);
    await until(() => errors.some(e => e.kind === 'status' && e.message.includes('not running')),
        'Missing status file was not reported');

    GLib.file_set_contents(path, '{"schema_version":1,"connected":true,"device_name":"Original"}');
    await until(() => statuses.at(-1)?.name === 'Original', 'New status file was not read');
    GLib.file_set_contents(path, '{"schema_version":1,"connected":true,"device_name":"Replacement"}');
    await until(() => statuses.at(-1)?.name === 'Replacement', 'Atomic status replacement was not read');

    const beforeMalformed = errors.length;
    const beforeRecovery = statuses.length;
    GLib.file_set_contents(path, '{invalid json');
    await until(() => errors.length > beforeMalformed, 'Malformed status was not reported');
    assert(errors.at(-1).kind === 'status', 'Malformed status was misclassified');
    GLib.file_set_contents(path, '{"schema_version":1,"connected":true,"device_name":"Replacement"}');
    await until(() => statuses.length > beforeRecovery && statuses.at(-1)?.name === 'Replacement',
        'Restoring identical valid contents after a parse error did not recover');

    const maximumBytes = 64 * 1024;
    const boundaryStatus = JSON.stringify({schema_version: 1, connected: true, device_name: 'Boundary'});
    const exactLimit = boundaryStatus.padEnd(maximumBytes, ' ');
    GLib.file_set_contents(path, exactLimit);
    await until(() => statuses.at(-1)?.name === 'Boundary', 'Valid status at the byte limit was rejected');
    const beforeOversized = errors.length;
    const beforeOversizedStatus = statuses.length;
    GLib.file_set_contents(path, `${exactLimit} `);
    await until(() => errors.length > beforeOversized, 'Oversized status was not rejected');
    assert(errors.at(-1).kind === 'status' && errors.at(-1).message.includes('size limit'),
        'Oversized status did not report a size error');
    assert(statuses.length === beforeOversizedStatus, 'Oversized status reached the UI');
    GLib.file_set_contents(path, exactLimit);
    await until(() => statuses.length > beforeOversizedStatus && statuses.at(-1)?.name === 'Boundary',
        'Identical valid status did not recover after oversized input');

    const beforeMultibyte = errors.length;
    GLib.file_set_contents(path, JSON.stringify({schema_version: 1, device_name: 'é'.repeat(maximumBytes / 2)}));
    await until(() => errors.length > beforeMultibyte, 'Status limit counted characters instead of bytes');
    assert(errors.at(-1).message.includes('size limit'), 'Multibyte oversized status had the wrong error');
    GLib.file_set_contents(path, '{"schema_version":1,"connected":true,"device_name":"Replacement"}');
    await until(() => statuses.at(-1)?.name === 'Replacement', 'Valid status did not recover after multibyte overflow');

    await sleep(100);
    const load = backend._readStatus.bind(backend);
    let reads = 0, activeReads = 0, peakReads = 0;
    backend._readStatus = async () => {
        reads++;
        peakReads = Math.max(peakReads, ++activeReads);
        try { return await load(); }
        finally { activeReads--; }
    };
    const beforeRefresh = statuses.length;
    for (let i = 0; i < 50; i++) backend.refresh();
    await until(() => !backend._reading, 'Refresh burst did not finish');
    assert(peakReads === 1 && reads < 50, 'Refresh burst started redundant overlapping reads');
    assert(statuses.length === beforeRefresh, 'Unchanged contents triggered another UI update');

    backend.refresh();
    const latestContents = '{"schema_version":1,"connected":true,"device_name":"Latest"}';
    GLib.file_set_contents(path, latestContents);
    for (let i = 0; i < 50; i++) backend.refresh();
    await until(() => statuses.at(-1)?.name === 'Latest', 'Coalescing lost the newest status during a read');
    assert(peakReads === 1, 'Concurrent file changes started overlapping reads');

    // A stale read error must not hide a newer atomic replacement.
    await sleep(100);
    const trackedLoad = backend._readStatus.bind(backend);
    let releaseRead;
    backend._readStatus = async () => {
        backend._readStatus = trackedLoad;
        await new Promise(resolve => { releaseRead = resolve; });
        throw new Error('AirPods input exceeds size limit');
    };
    const beforeStale = errors.length;
    backend.refresh();
    GLib.file_set_contents(path, '{"schema_version":1,"connected":true,"device_name":"Newest"}');
    backend.refresh();
    releaseRead();
    await until(() => statuses.at(-1)?.name === 'Newest', 'Stale oversized read lost the newer status');
    assert(errors.length === beforeStale, 'Superseded oversized read hid the newer status');

    await sleep(100);
    const beforeUnrelated = statuses.length + errors.length;
    GLib.file_set_contents(`${directory}/unrelated`, 'not a status update');
    await sleep(100);
    assert(statuses.length + errors.length === beforeUnrelated, 'Unrelated files triggered a status read');

    const beforeDeletion = errors.length;
    Gio.File.new_for_path(path).delete(null);
    await until(() => errors.length > beforeDeletion, 'Status deletion was not reported');
    assert(errors.at(-1).message.includes('not running'), 'Deleted status was not treated as unavailable');

    const beforeRestart = statuses.length;
    GLib.file_set_contents(path, latestContents);
    await until(() => statuses.length > beforeRestart && statuses.at(-1)?.name === 'Latest',
        'Service restart with identical contents did not restore the UI');

    backend.refresh();
    backend.destroy();
    backend.destroy();
    const beforeDestroy = statuses.length + errors.length;
    GLib.file_set_contents(path, '{"schema_version":1,"connected":true}');
    backend.refresh();
    await sleep(100);
    assert(statuses.length + errors.length === beforeDestroy, 'Disable delivered a stale status callback');
    print('PASS: missing status, atomic updates, byte limits, overflow recovery, coalesced reads, unchanged status filtering, latest snapshot, stale error suppression, error/restart recovery, file filtering, disable cleanup');
}
run().catch(error => { failure = error; }).finally(() => loop.quit());
loop.run();
backend?.destroy();
for (const entry of [path, `${directory}/unrelated`, directory, root]) {
    const file = Gio.File.new_for_path(entry);
    if (file.query_exists(null)) file.delete(null);
}
if (failure) throw failure;
