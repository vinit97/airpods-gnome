#!/usr/bin/env bash
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/common.sh"

if (($#)); then
    printf 'Usage: %s\nSet CARGO_BUILD_JOBS to choose the number of build jobs.\n' "$0" >&2
    exit 2
fi
require_commands cargo rustc cc
if [[ ! -f "$project_dir/daemon/Cargo.toml" || ! -f "$project_dir/daemon/Cargo.lock" ]]; then
    printf 'Bundled backend source is missing: %s/daemon\n' "$project_dir" >&2
    exit 1
fi
build_jobs=${CARGO_BUILD_JOBS:-8}
if [[ ! "$build_jobs" =~ ^[1-9][0-9]*$ ]]; then
    printf 'CARGO_BUILD_JOBS must be a positive integer.\n' >&2
    exit 2
fi

cargo_options=(--locked --release --manifest-path "$project_dir/daemon/Cargo.toml"
    --target-dir "$project_dir/build/daemon-rust" --jobs "$build_jobs")
cargo test "${cargo_options[@]}"
cargo build "${cargo_options[@]}" --bins
