#!/usr/bin/env bash
set -euo pipefail

export CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export CARGO_TARGET_DIR="$PWD/target"
mkdir -p "$CARGO_TARGET_DIR"
if ! command -v google-chrome-stable >/dev/null 2>&1; then
  curl -fsSL 'https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb' -o "$CARGO_TARGET_DIR/google-chrome.deb"
  sudo apt-get update
  sudo apt-get install -y "$CARGO_TARGET_DIR/google-chrome.deb"
  rm "$CARGO_TARGET_DIR/google-chrome.deb"
fi
browser="$(command -v google-chrome-stable)"
export MOMSO_TEST_CHROMIUM="$browser"
rustup component add rustfmt clippy
cargo fmt --manifest-path apps/alpha/server-browser/Cargo.toml -- --check
cargo clippy --manifest-path apps/alpha/server-browser/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path apps/alpha/server-browser/Cargo.toml -- --test-threads=1
cargo test --manifest-path apps/alpha/server-browser/Cargo.toml -- --ignored --test-threads=1
