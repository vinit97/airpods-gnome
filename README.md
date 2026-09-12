# AirPods for GNOME

AirPods battery levels and listening controls in the GNOME Shell 50 top bar.

Shared as-is, with no commitment to maintenance, support, or pull request reviews.
For fixes or new features, fork this repository and point your coding agent at
[AGENTS.md](AGENTS.md). Review and test changes in your fork.

![AirPods menu](docs/preview.png)

Preview uses sample data. [Light theme](docs/preview-light.png).

## Install

From the full project source on Fedora 44:

```bash
sudo dnf install cargo rust gcc python3 gjs pipewire-utils wireplumber
./setup
```

Run `./setup` as your desktop user, without sudo. It builds/tests the backend,
installs both components under `~/.local`, enables the extension, and starts
`airpods-gnome.service`. Run it again to update; saved settings are kept.

- **Build:** Rust/Cargo, GCC, Python 3, Bash, and `gnome-extensions`.
  Cargo downloads the libraries pinned in `daemon/Cargo.lock`.
- **Runtime:** GNOME Shell 50/GJS, BlueZ, PipeWire/WirePlumber (`pw-dump` and
  `wpctl`), D-Bus, and a systemd user session.

Log out and back in if GNOME has not discovered a first installation, or to load
updated extension code. Setup has already saved the extension's enabled state.
Pair your AirPods in GNOME Bluetooth Settings; the indicator appears when connected.

## Use

- Click a listening mode, or right-click/scroll the top-bar icon to cycle modes.
- Adjust the Adaptive slider or Conversation Awareness switch.
- **Ear Detection:** Off disables automatic pausing; I pauses when either AirPod
  is removed; II pauses when both are removed.

Battery rings show left, case, and right; AirPods Max shows one battery. Unknown
readings show a dash. Case readings can be stale; opening the lid with an AirPod
inside may refresh them. Available controls vary by model.

Ear Detection persists across restarts. Conversation Awareness and Adaptive
choices are remembered, but live AirPods reports take precedence. Listening mode
comes from the AirPods. See [behavior details](docs/behavior.md).

## Files and diagnostics

| File | Default location |
| --- | --- |
| Backend and command-line client | `~/.local/bin/` |
| User service | `~/.local/share/systemd/user/airpods-gnome.service` |
| Saved settings | `~/.config/AirPodsTrayApp/rust-settings.json` |
| Live status | `~/.local/state/airpods-gnome/status.json` |
| Command socket | `$XDG_RUNTIME_DIR/airpods-gnome.sock` |

Settings and state follow `XDG_CONFIG_HOME` and `XDG_STATE_HOME` when set.
Use `journalctl --user -u airpods-gnome.service -b` for logs and
`~/.local/bin/airpods-gnome-ctl status` for current readings.

## Remove

```bash
./uninstall
```

Run without sudo. Removes the extension, backend binaries, and user service.
Saved settings and the source project are kept.

## Development

Tests also require Node.js/npm; no npm dependencies are needed.

```bash
npm run check
npm test
```

`npm test` runs all automated suites, including backend lifecycle tests with
simulated AirPods. Optional `npm run test:shell` needs `gnome-shell-test-tool`,
`dbus-run-session`, and graphics access. Real battery and audio checks need AirPods.

Use `./setup --build-only` to build/test and package without installing.
`CARGO_BUILD_JOBS` defaults to eight; output goes in `build/` and `dist/`.
Regenerate icons from `assets/AirPodsIcon.qml` with
`python3 scripts/import-omapods-icons.py`.

## Credits and license

Inspired by [Omapods](https://github.com/thisisgm/omarchy-pods) and
[LibrePods](https://github.com/librepods-org/librepods).

[Extension license](LICENSE) · [Backend license](daemon/LICENSE) ·
[Upstream provenance](daemon/UPSTREAM.md) · [Artwork credits](icons/README.md).
