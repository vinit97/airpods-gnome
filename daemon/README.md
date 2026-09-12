# AirPods backend

Headless Rust backend for the GNOME extension. See the [project README](../README.md)
for installation, [behavior details](../docs/behavior.md), and
[upstream provenance](UPSTREAM.md).

Build and test from the project root:

```bash
bash scripts/build-backend.sh
npm run test:lifecycle
```

Requires Rust/Cargo, a C linker, Bash, and Python 3. Cargo uses the checked-in
lockfile and writes to `build/daemon-rust/`. Runtime dependencies are listed in
the project README; no Qt application is built or installed.

Lifecycle tests additionally use GJS and build with `test-support` in
`build/lifecycle/`. They exercise protocol packets, restarts, reconnects, and the
extension bridge with simulated AirPods. Installed binaries omit this transport.

The `airpods-gnome.service` user unit starts `~/.local/bin/airpods-gnome`.
The extension watches `$XDG_STATE_HOME/librepods/status.json` and sends commands
with `~/.local/bin/airpods-gnome-ctl` through `$XDG_RUNTIME_DIR/librepods.sock`.
State defaults to `~/.local/state`; settings use
`$XDG_CONFIG_HOME/AirPodsTrayApp/` (default `~/.config/AirPodsTrayApp/`).

```bash
systemctl --user status airpods-gnome.service
journalctl --user -u airpods-gnome.service -b
~/.local/bin/airpods-gnome-ctl status
```

Setup migrates the earlier `librepods.service` and removes known original files.
It refuses customized legacy services and preserves customized commands. A
`librepods-ctl` symlink supports an already loaded extension during migration;
remove it after a new login if desired. Later updates do not recreate a retired
alias. Legacy settings files are retained during import. Run only one backend at a time.
