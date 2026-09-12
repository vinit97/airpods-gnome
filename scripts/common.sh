#!/usr/bin/env bash
# Shared paths for the build, install, and isolated Shell test scripts.
project_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

require_commands() {
    local command_name
    local missing=()
    for command_name in "$@"; do
        command -v "$command_name" >/dev/null 2>&1 || missing+=("$command_name")
    done
    if ((${#missing[@]})); then
        printf 'Missing required commands: %s\nInstall the dependencies listed in README.md.\n' "${missing[*]}" >&2
        return 1
    fi
}

require_commands python3
extension_uuid=$(python3 - "$project_dir/metadata.json" <<'PY'
import json
import re
import sys

with open(sys.argv[1], encoding="utf-8") as metadata_file:
    uuid = json.load(metadata_file)["uuid"]
if not isinstance(uuid, str) or not re.fullmatch(r"[A-Za-z0-9._+-]+@[A-Za-z0-9._-]+", uuid):
    raise SystemExit("metadata.json must contain a valid extension UUID")
print(uuid)
PY
)
extension_bundle="$project_dir/dist/$extension_uuid.shell-extension.zip"
