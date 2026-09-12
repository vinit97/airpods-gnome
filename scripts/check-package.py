#!/usr/bin/env python3
"""Check that a release archive contains only the current extension files."""

from pathlib import Path
import sys
from zipfile import BadZipFile, ZipFile


def check_package(bundle):
    project_dir = Path(__file__).resolve().parent.parent
    expected = {
        "extension.js", "model.js", "backend.js", "stylesheet.css",
        "metadata.json", "LICENSE", "icons/README.md", "icons/LICENSE.omapods",
    }
    expected.update(path.relative_to(project_dir).as_posix()
                    for path in (project_dir / "icons").glob("*.svg"))

    with ZipFile(bundle) as archive:
        entries = archive.infolist()
        names = [entry.filename for entry in entries]
        if len(names) != len(set(names)):
            raise ValueError("duplicate archive entries")
        if any(entry.is_dir() and entry.filename != "icons/" for entry in entries):
            raise ValueError("unexpected archive directory")
        files = {entry.filename for entry in entries if not entry.is_dir()}
        missing, extra = expected - files, files - expected
        if missing or extra:
            raise ValueError(f"missing files: {sorted(missing)}; unexpected files: {sorted(extra)}")
        for name in sorted(expected):
            if archive.read(name) != (project_dir / name).read_bytes():
                raise ValueError(f"archive does not match source: {name}")
    print(f"Validated bundle: {bundle}")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("Usage: check-package.py BUNDLE")
    try:
        check_package(Path(sys.argv[1]))
    except (OSError, BadZipFile, ValueError) as error:
        raise SystemExit(f"Invalid extension bundle: {error}") from error
