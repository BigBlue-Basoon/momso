#!/usr/bin/env bash
set -euo pipefail
# Isolated test mount only; never touches the server's data directory.
test_mount="$(mktemp -d)"
mounted=0
cleanup() {
  if [ "$mounted" = 1 ]; then sudo -n umount "$test_mount"; fi
  rmdir "$test_mount"
}
trap cleanup EXIT
sudo -n mount -t tmpfs -o size=128m,mode=1777 tmpfs "$test_mount"
mounted=1
export MOMSO_S167_TEST_TMPFS="$test_mount"
cargo test -j 1 --features test-fixtures --test restart small_filesystem_refuses_upload_and_keeps_reads_available -- --test-threads=2
sudo -n umount "$test_mount"
mounted=0
sudo -n mount -t tmpfs -o size=2g,mode=1777 tmpfs "$test_mount"
mounted=1
unset MOMSO_S167_TEST_TMPFS
export MOMSO_S167_TEST_RECOVERY_TMPFS="$test_mount"
cargo test -j 1 --features test-fixtures --test restart full_filesystem_accepts_upload_after_space_is_freed -- --test-threads=2
