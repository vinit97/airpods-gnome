# Project conventions

- Keep personal names out of the README.
- Build from `daemon/` and regenerate icons from `assets/AirPodsIcon.qml`;
  never download another project's checkout during a build.
- Preserve upstream licenses and provenance. Output belongs in `build/` and
  `dist/`; extension ZIPs contain only runtime files and notices.
- Use native GNOME controls, 12px menu text, and a 15px title. Show the QML-derived
  battery rings in left, case, right order; hide the indicator when disconnected.
- Keep helper notes, tips, connection-status text, and errors out of the interface.
- Update controls immediately, preserve the latest choice through stale replies,
  and keep the slider under the pointer. On failure or timeout, quietly restore
  the reported value and log technical details.
