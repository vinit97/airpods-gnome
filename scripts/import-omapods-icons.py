#!/usr/bin/env python3
"""Convert the paths and ink bounds in Omapods' QML icon to GNOME symbolic SVGs."""
import hashlib
import math
import re
from pathlib import Path
from xml.sax.saxutils import escape


# Keep regenerated artwork aligned with the provenance recorded in icons/README.md.
SOURCE_COMMIT = 'fff7fec600a5b9a61cdb40e93eccbcceb4b8f824'
SOURCE_SHA256 = 'e0ca5e16d5b9213e1cc00c0baec2a0703187aff93646312f7ac034d541c6d603'


def render_icons(qml):
    """Validate the source and prepare every SVG before any files are changed."""
    # Match the ink bounds used by the QML component's fit/center transform.
    bounds = re.search(
        r'property var ink: isPro \? \[([^]]+)\]\s*'
        r': isMax \? \[([^]]+)\]\s*:\s*\[([^]]+)\]', qml)
    if bounds is None:
        raise ValueError('Unrecognized AirPodsIcon.qml ink bounds')
    icons = {}
    for variant, group in [('pro', 1), ('max', 2), ('buds', 3)]:
        paths = re.findall(rf'property string {variant}Path:\s*"([^"]*)"', qml)
        if len(paths) != 1 or not re.fullmatch(r'[Mm][0-9eE+.,\sMmLlHhVvCcSsQqTtAaZz-]+', paths[0]):
            raise ValueError(f'Missing or malformed {variant}Path in AirPodsIcon.qml')
        path = paths[0]
        try:
            x, y, width, height = [float(number) for number in bounds[group].split(',')]
        except ValueError:
            raise ValueError(f'Malformed {variant} ink bounds in AirPodsIcon.qml') from None
        if not all(math.isfinite(number) for number in (x, y, width, height)) or width <= 0 or height <= 0:
            raise ValueError(f'Invalid {variant} ink bounds: finite coordinates and positive dimensions required')
        side = max(width, height)
        view_box = f'{x - (side - width) / 2:g} {y - (side - height) / 2:g} {side:g} {side:g}'
        icons[f'airpods-{variant}-symbolic.svg'] = (
            '<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" '
            f'viewBox="{view_box}">\n'
            '  <!-- Artwork from Omapods AirPodsIcon.qml; see README.md in this directory. -->\n'
            f'  <path fill="#2e3436" fill-rule="nonzero" stroke="none" d="{escape(path)}"/>\n'
            '</svg>\n'
        )
        if variant == 'max':
            continue
        for ear, clip_x in [('left', x), ('right', x + width / 2)]:
            # Keep the original compound path and clip the corresponding earbud.
            side = max(width / 2, height)
            view_box = f'{clip_x - (side - width / 2) / 2:g} {y - (side - height) / 2:g} {side:g} {side:g}'
            icons[f'airpods-{variant}-{ear}-symbolic.svg'] = (
                '<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" '
                f'viewBox="{view_box}">\n'
                '  <!-- Clipped from Omapods AirPodsIcon.qml; see README.md in this directory. -->\n'
                f'  <defs><clipPath id="ear"><rect x="{clip_x:g}" y="{y:g}" width="{width / 2:g}" height="{height:g}"/></clipPath></defs>\n'
                f'  <path clip-path="url(#ear)" fill="#2e3436" fill-rule="nonzero" d="{escape(path)}"/>\n'
                '</svg>\n'
            )
    return icons


def import_icons(source, destination):
    source_bytes = source.read_bytes()
    icons = render_icons(source_bytes.decode('utf-8'))
    if hashlib.sha256(source_bytes).hexdigest() != SOURCE_SHA256:
        raise ValueError(f'AirPodsIcon.qml differs from attributed revision {SOURCE_COMMIT}; '
                         'review the source and update its attribution before importing')
    for name, svg in icons.items():
        (destination / name).write_text(svg, encoding='utf-8')
    return icons


def main():
    project = Path(__file__).resolve().parent.parent
    try:
        icons = import_icons(project / 'assets/AirPodsIcon.qml', project / 'icons')
    except (OSError, ValueError) as error:
        raise SystemExit(f'Could not import AirPods icons: {error}') from None
    for name in icons:
        print(f'icons/{name}')


if __name__ == '__main__':
    main()
