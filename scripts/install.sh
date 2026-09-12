#!/usr/bin/env bash
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/common.sh"
if (($#)); then
    printf 'Usage: bash scripts/install.sh\nUse ./setup to install the bundled backend too.\n' >&2
    exit 2
fi
require_commands gnome-extensions
bash "$project_dir/scripts/pack.sh"
gnome-extensions install --force "$extension_bundle"
printf 'Installed. Log out and back in, then run: gnome-extensions enable %q\n' "$extension_uuid"
