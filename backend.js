// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 AirPods for GNOME contributors
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import {parseStatus, validCommand} from './model.js';

const statusDecoder = new TextDecoder();

export function findControlExecutable(home = GLib.get_home_dir(), findInPath = name => GLib.find_program_in_path(name)) {
    const local = GLib.build_filenamev([home, '.local', 'bin', 'airpods-gnome-ctl']);
    if (GLib.file_test(local, GLib.FileTest.IS_EXECUTABLE)) return local;
    return findInPath('airpods-gnome-ctl');
}

export class Backend {
    constructor(onStatus, onError, stateHome = GLib.get_user_state_dir(), options = {}) {
        this._onStatus = onStatus;
        this._onError = onError;
        this._alive = true;
        this._reading = false;
        this._refreshAgain = false;
        this._lastContents = null;
        this._queue = [];
        this._process = null;
        this._commandCancel = null;
        this._timeout = 0;
        this._ctlPath = options.ctlPath;
        this._timeoutMs = options.timeoutMs ?? 5000;
        this._cancel = new Gio.Cancellable();
        const directory = GLib.build_filenamev([stateHome, 'airpods-gnome']);
        if (GLib.mkdir_with_parents(directory, 0o700) !== 0)
            throw new Error('Cannot access AirPods status directory');
        this._file = Gio.File.new_for_path(GLib.build_filenamev([directory, 'status.json']));
        // Watch the directory because the daemon replaces the file atomically.
        this._monitor = Gio.File.new_for_path(directory).monitor_directory(Gio.FileMonitorFlags.NONE, this._cancel);
        this._monitor.connect('changed', (_monitor, file, otherFile) => {
            if (file.equal(this._file) || otherFile?.equal(this._file))
                this.refresh();
        });
        this.refresh();
    }

    refresh() {
        if (!this._alive) return;
        // Atomic replacements can emit several events. Read once at a time,
        // then pick up the newest snapshot if anything changed during the read.
        if (this._reading) {
            this._refreshAgain = true;
            return;
        }
        this._reading = true;
        this._file.load_contents_async(this._cancel, (file, result) => {
            let status, contents, readError;
            try {
                const [, bytes] = file.load_contents_finish(result);
                if (this._alive && !this._refreshAgain) {
                    contents = statusDecoder.decode(bytes);
                    if (contents !== this._lastContents) status = parseStatus(contents);
                }
            } catch (error) {
                if (this._alive && !this._refreshAgain) {
                    // The same contents must be delivered again after a service
                    // outage or parse error, since the UI was hidden on failure.
                    this._lastContents = null;
                    const missing = error.matches?.(Gio.io_error_quark(), Gio.IOErrorEnum.NOT_FOUND);
                    readError = missing ? 'AirPods service is not running' : error.message;
                }
            }
            // UI exceptions must not be reported as a failure to read the service.
            try {
                if (status) {
                    this._onStatus(status);
                    this._lastContents = contents;
                } else if (readError) this._onError(readError, 'status');
            } finally {
                this._reading = false;
                if (this._refreshAgain) {
                    this._refreshAgain = false;
                    this.refresh();
                }
            }
        });
    }

    command(verb, selection = null) {
        if (!this._alive || !validCommand(verb)) return false;
        // Preserve different controls; replace queued slider/mode changes with the latest.
        const key = verb.split(':')[0];
        const index = this._queue.findIndex(command => command.verb.startsWith(`${key}:`));
        const command = {verb, selection};
        if (index >= 0) this._queue[index] = command;
        else this._queue.push(command);
        this._runNext();
        return true;
    }

    _runNext() {
        if (!this._alive || this._process || !this._queue.length) return;
        const command = this._queue.shift();
        const {verb, selection} = command;
        const executable = this._ctlPath ?? findControlExecutable();
        if (!executable) {
            this._failCommands('AirPods backend is not installed', command);
            return;
        }
        let process;
        try {
            process = Gio.Subprocess.new([executable, verb], Gio.SubprocessFlags.STDOUT_SILENCE | Gio.SubprocessFlags.STDERR_PIPE);
        } catch (error) {
            this._failCommands(error.message, command);
            return;
        }
        this._process = process;
        const cancel = new Gio.Cancellable();
        this._commandCancel = cancel;
        let timedOut = false;
        this._timeout = GLib.timeout_add(GLib.PRIORITY_DEFAULT, this._timeoutMs, () => {
            this._timeout = 0;
            timedOut = true;
            process.force_exit();
            // A descendant can keep stderr open even after the direct process exits.
            cancel.cancel();
            return GLib.SOURCE_REMOVE;
        });
        process.communicate_utf8_async(null, cancel, (proc, result) => {
            let message;
            try {
                const [, , stderr] = proc.communicate_utf8_finish(result);
                if (!proc.get_successful())
                    message = stderr.trim().slice(0, 160) || 'AirPods command failed';
            } catch (error) {
                message = error.message;
            }
            if (timedOut) message = 'AirPods command timed out';
            if (this._timeout) GLib.Source.remove(this._timeout);
            this._timeout = 0;
            this._process = null;
            this._commandCancel = null;
            if (!this._alive) return;
            // Keep UI exceptions separate from command failures, and keep the queue moving.
            try {
                if (message) this._onError(message, 'command', verb, selection);
                else this.refresh();
            } finally {
                this._runNext();
            }
        });
    }

    _failCommands(message, command) {
        // Every discarded command needs a failure so its optimistic control resets.
        const commands = [command, ...this._queue.splice(0)];
        for (const {verb, selection} of commands) {
            if (!this._alive) break;
            this._onError(message, 'command', verb, selection);
        }
    }

    destroy() {
        if (!this._alive) return;
        this._alive = false;
        this._refreshAgain = false;
        this._lastContents = null;
        this._queue = [];
        this._cancel.cancel();
        this._commandCancel?.cancel();
        this._commandCancel = null;
        this._monitor.cancel();
        if (this._timeout) GLib.Source.remove(this._timeout);
        this._timeout = 0;
        this._process?.force_exit();
        this._process = null;
    }
}
