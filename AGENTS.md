# Working on AirPods for GNOME

This repository contains a GNOME Shell extension and a Rust backend for AirPods
battery readings, listening modes, Conversation Awareness, and Ear Detection.
The current target is Fedora 44 with GNOME Shell 50. Read [README.md](README.md)
for dependencies and setup, and [docs/behavior.md](docs/behavior.md) for expected
behavior before changing a feature.

The project is shared as-is. Make and validate changes in your fork; do not assume
upstream support, releases, or pull request reviews.

## Source map

| Location | Responsibility |
| --- | --- |
| `extension.js`, `stylesheet.css` | Panel indicator, menu, battery rings, controls, and immediate visual feedback. |
| `model.js` | Shell-independent status parsing, capability filtering, and command validation. |
| `backend.js` | Watches the status file and queues asynchronous calls to `airpods-gnome-ctl`. |
| `daemon/src/main.rs` | Backend lifecycle, command handling, status publication, and coordination. |
| `daemon/src/bluetooth.rs`, `protocol.rs`, `model.rs` | BlueZ discovery and reconnection, AirPods packets, battery readings, and device capabilities. |
| `daemon/src/media.rs` | PipeWire/WirePlumber audio handling and MPRIS playback control. |
| `daemon/src/settings.rs`, `ipc.rs`, `bin/ctl.rs` | Saved preferences, private files and socket, daemon lock, and command-line client. |
| `setup`, `uninstall`, `scripts/`, `daemon/airpods-gnome.service` | Build, packaging, user installation, removal, and service lifecycle. |
| `assets/`, `icons/`, `tests/` | Bundled icon source, generated artwork and notices, and automated checks. |

The daemon publishes JSON status; the extension monitors its directory because
updates replace the file atomically. Commands travel through the client to a Unix
socket. Keep device and audio logic in Rust, and presentation in the extension.

## Behavior to preserve

- Show the indicator when the AirPods control connection is ready; hide it on
  disconnect or unavailable status. Reconnection remains the backend's job.
- Keep left, right, and case battery readings independent. Unknown values show a
  dash; never substitute another component's percentage. Case data may remain
  stale until another transmission. AirPods Max uses one headset battery.
- Offer only controls supported by the connected model. Keep listening-mode
  buttons, top-bar scroll/right-click cycling, and the Adaptive slider.
- Use a native switch for Conversation Awareness and an Off / I / II selector
  for Ear Detection. Preserve keyboard navigation and accessible labels.
- Update selections immediately. A stale report or earlier command failure must
  not undo a newer choice. Coalesce queued changes per control, keep the Adaptive
  slider under the pointer, and quietly restore reported values on failure or
  timeout. Technical errors belong in logs.
- Ear Detection I pauses when either earbud is removed; II pauses when both are
  removed. Resume only players the backend paused. Preserve microphone use,
  manual playback choices, and the settling behavior in `docs/behavior.md`.
- Conversation Awareness lowers playback volume while music continues. Restore
  volume only when the output and volume still match the backend's intervention;
  preserve manual volume changes and the existing retry behavior.
- Retain Ear Detection across restarts. Conversation Awareness and Adaptive
  preferences are remembered, but live device reports take precedence. Listening
  mode comes from the AirPods.

## Implementation conventions

- Keep changes focused on the requested functionality and its tests. Prefer the
  existing modules over new frameworks, dependencies, or compatibility layers.
- Build the Rust source in `daemon/` with the committed `Cargo.lock`; never
  download another project's checkout during a build. Python supports build and
  test tooling; the installed backend is Rust.
- Preserve the status schema and command contract across Rust and JavaScript.
  Ear Detection values are `0 = I`, `1 = II`, and `2 = Off`, regardless of display
  order. Update both sides and their tests when changing an interface.
- Paths in the README are active interfaces. Coordinate path changes across the
  daemon, client, extension, service, and tests. Preserve XDG handling, private
  file permissions, atomic writes, and the single-daemon lock.
- Preserve unknown JSON settings fields. Invalid settings must not silently
  overwrite saved preferences with defaults.
- Keep Shell I/O asynchronous. Release monitors, signals, timers, and subprocesses
  on disable so re-enabling the extension does not duplicate work. Backend
  shutdown must retain audio restoration and runtime-file cleanup.
- Keep fresh installation straightforward and preserve settings on updates and
  uninstall. Avoid adding obsolete migration paths or command aliases.
- Put generated build output in `build/` and packages in `dist/`. Extension ZIPs
  contain only runtime files and notices, as checked by `scripts/check-package.py`.

## Interface and artwork

Use native GNOME controls, a compact layout, 12px menu text, and a 15px title.
Show circular battery percentages in left, case, right order. Keep helper notes,
tips, connection-status text, errors, and Bluetooth-settings shortcuts out of the
menu. Preserve the labels “Noise Cancellation” and “Ear Detection”.

Generate product icons from the bundled `assets/AirPodsIcon.qml` using
`python3 scripts/import-omapods-icons.py`. GNOME renders the resulting SVGs; QML is
source artwork, not a runtime dependency. Preserve the artwork notices.

For layout changes, inspect both themes and update the README previews from the
isolated GNOME test:

```bash
mkdir -p build/previews
AIRPODS_SCREENSHOT_DIR="$PWD/build/previews" npm run test:shell
```

After inspecting the output, copy `build/previews/menu.png` to `docs/preview.png`
and `build/previews/menu-light.png` to `docs/preview-light.png`. Use sample device
data with distinct earbud percentages and a visible case percentage.

## Validation

Run commands from the repository root. Start with checks relevant to the change;
use `npm test` for changes spanning components. Extend existing tests for behavior
changes and regressions, without adding tests that merely repeat implementation.

| Command | Coverage |
| --- | --- |
| `npm run check` | JavaScript, Bash, and Python syntax; does not check Rust or render GNOME. |
| `npm run test:daemon` | Rust tests and release builds of both binaries. |
| `npm run test:model` | Status parsing, capabilities, and command validation. |
| `npm run test:backend` | GJS status monitoring and command queue behavior. |
| `npm run test:icons` | Icon generation from the bundled source. |
| `npm run test:installer` / `npm run test:uninstaller` | Installation and removal with isolated fixtures. |
| `npm run test:lifecycle` | Backend lifecycle with simulated AirPods; builds with `test-support`. |
| `npm run test:shell` | Isolated GNOME menu and interaction checks; requires `gnome-shell-test-tool`, `dbus-run-session`, and graphics access. |
| `npm run pack` | Extension ZIP creation and content validation. |

For Rust changes, also check formatting and linting:

```bash
cargo fmt --manifest-path daemon/Cargo.toml --check
cargo clippy --locked --manifest-path daemon/Cargo.toml --target-dir build/daemon-rust --all-targets --features test-support -- -D warnings
```

`npm test` runs the automated suites except the optional Shell test. Simulated
tests do not prove real Bluetooth battery reporting or audio behavior. Report
which checks ran and any hardware-dependent behavior left unverified.

## Installation and documentation

- Use `./setup --build-only` to build, test the backend, and package without
  changing the desktop installation. `./setup` installs and starts the user
  service and enables the extension; `./uninstall` removes installed components
  while retaining settings and source. Run installation/removal as the desktop
  user, without sudo, when that work is requested.
- Restarting the backend does not reload extension JavaScript. GNOME may need a
  new login to discover an extension or load updated code; use the isolated Shell
  test for development previews.
- Keep README instructions concise and update `docs/behavior.md` when behavior
  changes. Keep the public sharing expectations above intact unless requested.
- Use neutral project identifiers and contributor attribution. Keep personal
  names, usernames, home-directory paths, device identities, and pairing keys out
  of source, comments, fixtures, documentation, screenshots, and metadata.
- Preserve licenses and upstream attribution, including named authors in
  `daemon/UPSTREAM.md`, `assets/README.md`, and `icons/README.md`. Privacy cleanup
  must not remove third-party credits.
