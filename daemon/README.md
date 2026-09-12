# AirPods backend

This Rust daemon provides the Bluetooth connection, battery readings, listening
modes, Conversation Awareness, and ear detection used by the GNOME extension.
It implements the LibrePods protocol and preserves the existing GNOME integration.
See [UPSTREAM.md](UPSTREAM.md) for provenance and licensing.

Use the installation instructions in the [project README](../README.md) to set
up the extension and backend together. To build and test only the backend from
the project root:

```bash
bash scripts/build-backend.sh
npm run test:lifecycle
```

Build dependencies are Cargo, Rust, and a C linker. Cargo uses the checked-in
lockfile and writes output to `build/daemon-rust/`. Runtime integration uses
BlueZ, the user D-Bus session, and WirePlumber/PipeWire's `wpctl` and `pw-dump`.
Tests run without connected AirPods. No Qt application is built or installed.

The lifecycle suite compiles a separate binary with the `test-support` feature
in `build/lifecycle/`. It exchanges real protocol packets with a local simulator,
exercises daemon restarts and reconnects, and checks the GJS extension bridge.
The installed release binary does not include the simulated transport.

The installed `airpods-gnome.service` starts `airpods-gnome` in the user session.
The extension watches `$XDG_STATE_HOME/librepods/status.json` (default
`~/.local/state/librepods/status.json`) and sends commands through `airpods-gnome-ctl`
to `$XDG_RUNTIME_DIR/librepods.sock`.

```bash
systemctl --user status airpods-gnome.service
journalctl --user -u airpods-gnome.service -b
airpods-gnome-ctl status
```

Setup migrates the earlier `librepods.service` and removes its known original
daemon files. During a legacy upgrade, a `librepods-ctl` compatibility symlink keeps
the already loaded GNOME extension working. After a new login it can be removed;
later updates do not recreate a retired alias. Customized legacy commands remain
untouched. The status
directory and socket retain their names. Run only one backend at a time. Configuration is
stored in `$XDG_CONFIG_HOME/AirPodsTrayApp/` (default `~/.config/AirPodsTrayApp/`);
legacy configuration files are preserved when settings are imported.
