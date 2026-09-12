# Project conventions

- Keep personal names out of the README.
- Keep setup self-contained: build from `daemon/` and regenerate icons from
  `assets/AirPodsIcon.qml`. Do not download another project's checkout at build time.
- Preserve upstream licenses and provenance. Keep build and package output in `build/`
  and `dist/`; the extension ZIP contains only its runtime files and notices.
- Keep the menu compact: 12px text, 15px title, and native GNOME controls.
- Show battery rings in left, case, right order using the AirPodsIcon.qml artwork.
- Hide the top-bar indicator when disconnected.
- Do not add helper notes, tips, or connection-status text to the interface.
- Keep errors out of the menu. Quietly restore the reported control value and log technical details.
- Update controls immediately. Preserve the latest choice through stale replies,
  keep the slider under the pointer during dragging, and restore the reported
  value on failure or timeout.
