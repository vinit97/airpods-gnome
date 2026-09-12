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

enable_extension() {
    # Persist the choice even before GNOME discovers a first-time installation.
    # These are the same per-extension settings used by GNOME's enable command.
    gjs -c '
const Gio = imports.gi.Gio;
const settings = new Gio.Settings({schema_id: "org.gnome.shell"});
const uuid = ARGV[0];
const enabled = settings.get_strv("enabled-extensions");
const disabled = settings.get_strv("disabled-extensions");
settings.delay();
if (!enabled.includes(uuid) && !settings.set_strv("enabled-extensions", [...enabled, uuid]))
    throw new Error("Cannot enable the AirPods extension in GNOME settings");
if (disabled.includes(uuid) && !settings.set_strv("disabled-extensions", disabled.filter(id => id !== uuid)))
    throw new Error("Cannot clear the AirPods extension disable setting");
settings.apply();
Gio.Settings.sync();
' "$extension_uuid"
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
