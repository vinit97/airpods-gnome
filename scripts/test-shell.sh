#!/usr/bin/env bash
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/common.sh"
test_dir=$(mktemp -d -t airpods-shell-test-XXXXXX)
trap 'rm -rf -- "$test_dir"' EXIT
bash "$project_dir/scripts/pack.sh"
AIRPODS_EXTENSION_UUID="$extension_uuid" XDG_STATE_HOME="$test_dir/state" \
    dbus-run-session -- gnome-shell-test-tool \
    --headless --disable-animations \
    --extension "$extension_bundle" \
    "$project_dir/tests/shell-smoke.js"
