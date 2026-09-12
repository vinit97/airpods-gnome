#!/usr/bin/env bash
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/common.sh"
require_commands gnome-extensions
mkdir -p "$project_dir/dist"
gnome-extensions pack "$project_dir" --force --out-dir="$project_dir/dist" \
    --extra-source=model.js --extra-source=backend.js --extra-source=icons --extra-source=LICENSE
if ! python3 "$project_dir/scripts/check-package.py" "$extension_bundle"; then
    rm -f -- "$extension_bundle"
    exit 1
fi
