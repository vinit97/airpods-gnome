# Backend provenance

This directory is a Rust port of the Linux LibrePods backend previously bundled
in this project. LibrePods provides the AirPods protocol implementation and device
support; Omapods added the desktop-panel integration inherited here. This is a
local port, separate from LibrePods' own Rust development.

| Source | Revision |
| --- | --- |
| [LibrePods](https://github.com/kavishdevar/librepods), by Kavish Devar | `29a914c` (2026-05-19), as recorded by Omapods |
| [Omapods](https://github.com/thisisgm/omarchy-pods), by GM | `fff7fec600a5b9a61cdb40e93eccbcceb4b8f824`, `daemon/` subtree |

Omapods imported LibrePods' Linux subtree without its `extras/` developer tools.
The Android application and root module are not included. The inherited backend
changes include the live JSON status file, control commands for Conversation
Awareness and ear detection, Adaptive-level control, model and capability
updates, case-lid reporting, a runtime-directory socket, desktop notifications,
headless operation, a systemd user service, and connection reliability fixes.

## Local integration

AirPods GNOME builds the backend and extension from one repository. The Rust
implementation uses `airpods-gnome`, `airpods-gnome-ctl`, and
`airpods-gnome.service`. It preserves the status JSON, command socket, and
configuration directory, with a `librepods-ctl` alias for an already loaded
GNOME extension during migration. Protocol and behavior tests are accompanied
by process lifecycle tests that use an isolated transport.

The backend is headless. Qt windows, translations, GUI resources, and the QR-code
generator are not required by the GNOME interface and are omitted from the Rust
build. The GNOME extension and its QML-derived SVG icon artwork remain separate.
Cargo dependencies are pinned in `Cargo.lock`.

## Licenses

The LibrePods-derived backend remains under the GNU General Public License v3.0;
see [LICENSE](LICENSE). Original attribution and upstream source revisions are
recorded above. Third-party Rust dependencies retain their own licenses.
The earlier C++ QR Code generator was MIT-licensed;
the Rust backend does not use it. Artwork and trademarks retain their original
owners' rights; including source does not relicense third-party assets.

The GNOME extension has its own license and artwork notices in the project root.
Omapods' separate panel widget is not included in this directory.
