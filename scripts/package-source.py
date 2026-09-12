#!/usr/bin/env python3
"""Package the complete local source tree without build output or external checkouts."""

import gzip
import json
import os
from pathlib import Path
import tarfile
import tempfile


PROJECT = Path(__file__).resolve().parent.parent
FILES = (
    '.gitignore', 'AGENTS.md', 'LICENSE', 'README.md', 'package.json', 'setup', 'uninstall',
    'extension.js', 'backend.js', 'model.js', 'stylesheet.css', 'metadata.json',
)
DIRECTORIES = ('assets', 'daemon', 'docs', 'icons', 'scripts', 'tests')
EXCLUDED = {'.git', '__pycache__', 'build', 'target', 'node_modules'}


def raise_walk_error(error):
    raise error


def source_files():
    paths = [PROJECT / name for name in FILES]
    for name in DIRECTORIES:
        directory = PROJECT / name
        if directory.is_symlink() or not directory.is_dir():
            raise ValueError(f'Missing source directory: {name}')
        for root, directories, filenames in os.walk(directory, onerror=raise_walk_error):
            # Prune build trees before descending into their potentially large contents.
            directories[:] = [entry for entry in directories if entry not in EXCLUDED]
            for entry in directories + filenames:
                if entry in EXCLUDED:
                    continue
                path = Path(root) / entry
                if path.is_symlink():
                    raise ValueError(f'Source symlinks are not supported: {path.relative_to(PROJECT)}')
                if path.suffix not in {'.pyc', '.pyo'} and not path.is_dir():
                    paths.append(path)
    for path in sorted(paths):
        if path.is_symlink() or not path.is_file():
            raise ValueError(f'Expected a regular source file: {path.relative_to(PROJECT)}')
        yield path


def package_source():
    version = json.loads((PROJECT / 'metadata.json').read_text())['version']
    if not isinstance(version, int) or isinstance(version, bool) or version < 1:
        raise ValueError('metadata.json must have a positive integer version')
    paths = list(source_files())
    dist = PROJECT / 'dist'
    dist.mkdir(exist_ok=True)
    destination = dist / f'airpods-gnome-{version}-source.tar.gz'
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=dist, suffix='.tmp', delete=False) as output:
            temporary = Path(output.name)
            # Stable metadata also keeps local usernames and paths out of the archive.
            with gzip.GzipFile(fileobj=output, mode='wb', filename='', mtime=0) as compressed:
                with tarfile.open(fileobj=compressed, mode='w') as archive:
                    for path in paths:
                        name = f'airpods-gnome/{path.relative_to(PROJECT).as_posix()}'
                        info = archive.gettarinfo(str(path), arcname=name)
                        info.uid = info.gid = info.mtime = 0
                        info.uname = info.gname = ''
                        info.mode = 0o755 if path.stat().st_mode & 0o111 else 0o644
                        with path.open('rb') as source:
                            archive.addfile(info, source)
        temporary.replace(destination)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    print(f'Packaged {len(paths)} source files: {destination}')


if __name__ == '__main__':
    try:
        package_source()
    except (OSError, ValueError, KeyError, tarfile.TarError) as error:
        raise SystemExit(f'Cannot package source: {error}') from error
