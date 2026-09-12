# Backend provenance

This is a local Rust port of the previously bundled Linux LibrePods backend,
separate from LibrePods' own Rust development. LibrePods supplied the protocol and
device support; Omapods added desktop-panel integration.

| Source | Revision |
| --- | --- |
| [LibrePods](https://github.com/kavishdevar/librepods), by Kavish Devar | `29a914c` (2026-05-19), as recorded by Omapods |
| [Omapods](https://github.com/thisisgm/omarchy-pods), by GM | `fff7fec600a5b9a61cdb40e93eccbcceb4b8f824`, `daemon/` subtree |

Omapods imported the Linux subtree without `extras/`. Its additions included JSON
status and control commands, Conversation Awareness and ear-detection controls,
Adaptive levels, model capabilities, case-lid reports, a runtime socket,
notifications, headless operation, a user service, and connection fixes.

## Local integration

The Rust binaries, service, status directory, and runtime socket use the
`airpods-gnome` name. The status format and saved-settings directory are retained.
See the [project README](../README.md) for paths.
Cargo dependencies are pinned in `Cargo.lock`.

Qt windows, translations, GUI resources, the QR-code generator, LibrePods' Android
application and root module, and Omapods' panel widget are not included. The GNOME
extension and QML-derived SVG artwork are maintained separately in the project root.

## Licenses

The LibrePods-derived backend remains under the GNU General Public License v3.0;
see [LICENSE](LICENSE). Third-party Rust dependencies retain their own licenses.
The earlier C++ QR Code generator was MIT-licensed and is unused by this port.
The GNOME extension has separate [license](../LICENSE) and
[artwork notices](../icons/README.md). Artwork and trademarks retain their owners'
rights; including source does not relicense third-party assets.
