#!/bin/sh
# Prepares a new worktree for work. T3 Code runs this through t3.json, and it
# also works by hand.
set -eu

cd "$(dirname "$0")/.."

# The first cargo command installs the toolchain that rust-toolchain.toml
# pins. This one also downloads what Cargo.lock names.
cargo fetch --locked
