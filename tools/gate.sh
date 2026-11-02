#!/usr/bin/env bash
# Per-repo gate entry point. Declares which toolchains this repo contains and
# delegates; it holds no gate logic of its own.
#   --staged : pre-commit scope (fast) — layer 1 only
#   --full   : pre-push scope — every layer
set -euo pipefail
GOH="${GOH_DIR:-${GOH:-$HOME/Projects/gates_of_heck}}"

# Informational flags never reach the gates below.
case "${1:-}" in
  -h|--help)
    echo "usage: gate.sh [--staged | --full | --doctor [repo] | --help]"
    exit 0 ;;
  --doctor)  exec bash "$GOH/gates/doctor.sh" "${2:-$PWD}" ;;
esac

"$GOH/gates/structural.sh" "$@"

# The cheap half of a language gate runs at COMMIT time on the staged files
# (rule 14: a commit gate weaker than the push gate lets you commit red and
# find out at push). Uncomment for the languages this repo contains:
#   "$GOH/gates/py_staged.sh"  .

case "${1:-}" in
  --full)
    # The machine-wide gate slot (one helper, in goh): a full gate waits its turn beside every
    # other on this machine, and reuses its push gate's slot when nested in one.
    . "$GOH/tui/lib.sh"
    . "$GOH/lib/gate_slot.sh"
    trap gate_slot_release EXIT
    gate_slot_acquire "$(basename "$PWD")"
    # Add per-language layers for what this repo actually contains:
    # The Rust layer: fmt, pedantic clippy, cargo audit (deny list in .cargo/audit.toml),
    # machete, no-#[allow], and the tests under the coverage floor in .gatesrc.
    "$GOH/gates/rust_gate.sh"  .
    #   "$GOH/gates/py_gate.sh"    .
    #   "$GOH/gates/swift_gate.sh" .
    # Layer 3 (genuinely local checks): create ./tools/repo_gates.sh and
    # uncomment:
    #   ./tools/repo_gates.sh
    ;;
esac
