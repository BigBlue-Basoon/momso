#!/bin/sh
set -eu
fixture=$(mktemp -d /tmp/momso-s167-test.XXXXXX)
mounted=0
cleanup() {
    if [ "$mounted" = 1 ]; then sudo umount "$fixture"; fi
    rmdir "$fixture"
}
trap cleanup EXIT
sudo mount -t tmpfs -o size=2G tmpfs "$fixture"
mounted=1
sudo chown "$(id -u):$(id -g)" "$fixture"
export MOMSO_S167_TEST_RECOVERY_TMPFS="$fixture"
cargo test --features test-fixtures --test restart old_browser_cache_is_removed_and_upload_recovers -- --test-threads=1
