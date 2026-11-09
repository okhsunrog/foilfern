#!/usr/bin/env bash
# Run unmodified upstream tests for the native CLI's implemented commands.
# The full suite also includes deferred open-step/minification and Qt help snapshots.
set -euo pipefail
repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
upstream_dir="${LIBREPCB_UPSTREAM_DIR:-"$repo_dir/../LibrePCB"}"
upstream_dir="$(cd -- "$upstream_dir" && pwd)"
cd -- "$repo_dir"
export LIBREPCB_UPSTREAM_DIR="$upstream_dir"
cargo build -p librepcb-cli
cli_target_dir="$(cargo metadata --no-deps --format-version 1 | uv run --no-project python -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
cli_executable="$cli_target_dir/debug/librepcb-cli"
cd -- "$upstream_dir/tests/cli"
uv run pytest --librepcb-executable="$cli_executable" "$@" \
    open_project/test_open.py \
    open_project/test_save.py \
    open_project/test_strict.py \
    open_project/test_run_erc.py \
    open_project/test_run_drc.py \
    open_project/test_remove_other_boards.py \
    open_project/test_set_default_variant.py \
    open_project/test_board_filter.py \
    open_project/test_export_generic_bom.py \
    open_project/test_export_board_bom.py \
    open_project/test_export_pnp.py \
    open_project/test_export_netlist.py \
    open_project/test_export_pcb_fabrication_data.py \
    open_library/test_open.py \
    open_library/test_save.py \
    open_library/test_strict.py \
    open_library/test_check.py \
    open_symbol/test_check.py \
    open_package/test_check.py \
    open_symbol/test_export.py \
    open_package/test_export.py \
    open_project/test_export_schematics.py \
    open_project/test_run_jobs.py
