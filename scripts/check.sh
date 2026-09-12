#!/usr/bin/env bash
set -euo pipefail
project_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$project_dir"

for script in ./*.js tests/*.js; do
    node --check "$script"
done
for script in setup uninstall scripts/*.sh; do
    bash -n "$script"
done
python3 - <<'PY'
from pathlib import Path

for directory in (Path("scripts"), Path("tests")):
    for path in directory.glob("*.py"):
        compile(path.read_bytes(), str(path), "exec")
PY
printf 'JavaScript, Bash, and Python syntax checks passed.\n'
