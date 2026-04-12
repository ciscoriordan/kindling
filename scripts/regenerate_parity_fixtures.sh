#!/usr/bin/env bash
#
# Regenerate the kindlegen reference .mobi fixtures used by
# tests/kindlegen_parity.rs. Run this whenever a source fixture file
# (OPF, HTML, CSS, JPEG, CBZ, etc.) changes under
# tests/fixtures/parity/<name>/.
#
# kindlegen is Amazon-proprietary and cannot be committed to the repo.
# This script requires a local install. Set $KINDLEGEN_PATH to point at
# the binary, or place it on $PATH, or install it at $HOME/.local/bin/kindlegen.
#
# The generated OUTPUT (a Kindle-format binary built from our own
# fixture content) is committed alongside the sources so that the parity
# tests run without any kindlegen dependency.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PARITY="$ROOT/tests/fixtures/parity"

# Locate kindlegen. Mirrors the Rust-side lookup in tests/common/mod.rs.
find_kindlegen() {
  if [ -n "${KINDLEGEN_PATH:-}" ] && [ -x "$KINDLEGEN_PATH" ]; then
    echo "$KINDLEGEN_PATH"
    return 0
  fi
  if command -v kindlegen >/dev/null 2>&1; then
    command -v kindlegen
    return 0
  fi
  if [ -x "$HOME/.local/bin/kindlegen" ]; then
    echo "$HOME/.local/bin/kindlegen"
    return 0
  fi
  return 1
}

KG="$(find_kindlegen)" || {
  echo "error: kindlegen not found. Set KINDLEGEN_PATH or install at ~/.local/bin/kindlegen." >&2
  exit 2
}

echo "Using kindlegen at: $KG"

run_kg() {
  local fixture="$1"
  local opf_name="$2"
  local src_dir="$PARITY/$fixture"
  local work_dir
  work_dir="$(mktemp -d -t "kindling_regen_${fixture}_XXXX")"
  trap 'rm -rf "$work_dir"' RETURN

  # Copy everything EXCEPT the old kindlegen reference into the scratch dir.
  # Using a subshell + find so we don't drag in stale .mobi from a prior run.
  (cd "$src_dir" && find . -type f ! -name 'kindlegen_reference.mobi' -print0 | \
    xargs -0 -I{} cp --parents "{}" "$work_dir" 2>/dev/null) || \
  (cd "$src_dir" && for f in $(find . -type f ! -name 'kindlegen_reference.mobi'); do
    mkdir -p "$work_dir/$(dirname "$f")"
    cp "$f" "$work_dir/$f"
  done)

  "$KG" "$work_dir/$opf_name" -c0 >/tmp/kindling_regen.log 2>&1 || true
  local mobi_name="${opf_name%.opf}.mobi"
  local mobi_name_epub="${opf_name%.epub}.mobi"
  local produced=""
  if [ -f "$work_dir/$mobi_name" ]; then
    produced="$work_dir/$mobi_name"
  elif [ -f "$work_dir/$mobi_name_epub" ]; then
    produced="$work_dir/$mobi_name_epub"
  else
    echo "error: kindlegen did not produce an output for $fixture" >&2
    cat /tmp/kindling_regen.log >&2
    return 1
  fi
  cp "$produced" "$src_dir/kindlegen_reference.mobi"
  echo "wrote $src_dir/kindlegen_reference.mobi ($(wc -c < "$src_dir/kindlegen_reference.mobi") bytes)"
}

run_kg simple_dict simple_dict.opf
run_kg simple_book simple_book.opf
run_kg simple_comic simple_comic.epub

echo
echo "All parity references regenerated. Don't forget to commit:"
echo "  git add tests/fixtures/parity/*/kindlegen_reference.mobi"
