# AirPods product artwork

The `airpods-buds-symbolic.svg`, `airpods-pro-symbolic.svg` and
`airpods-max-symbolic.svg` files contain the vector paths from
[Omapods' AirPodsIcon.qml](https://github.com/thisisgm/omarchy-pods/blob/fff7fec600a5b9a61cdb40e93eccbcceb4b8f824/AirPodsIcon.qml).
The paths use the same ink bounds, centered square fitting and nonzero fill rule
as the QML component. They are rendered as symbolic SVG icons by GNOME instead of Qt.
The `buds-left`, `buds-right`, `pro-left`, and `pro-right` variants clip the same
outlines to show each earbud inside its battery ring. The case outline and
charging mark are original SVGs in this project, covered by GPL-3.0-or-later.

Upstream identifies these outlines as Apple's artwork from the product navigation
on apple.com/airpods. Its MIT license explicitly excludes the artwork. These
assets are likewise not covered by this extension's GPL license; their inclusion
does not grant redistribution rights to Apple's artwork. The upstream notice is
in its [README](https://github.com/thisisgm/omarchy-pods/blob/fff7fec600a5b9a61cdb40e93eccbcceb4b8f824/README.md#licence).
AirPods, AirPods Pro and AirPods Max are Apple trademarks. This extension is not
affiliated with or endorsed by Apple.

The upstream software license is preserved in [LICENSE.omapods](LICENSE.omapods) for the source
component and its fitting logic; it does not license the product outlines.

The original QML is bundled in [assets/AirPodsIcon.qml](../assets/AirPodsIcon.qml),
with its revision and checksum in [assets/README.md](../assets/README.md).
To regenerate from this local source:

```bash
python3 scripts/import-omapods-icons.py
```
