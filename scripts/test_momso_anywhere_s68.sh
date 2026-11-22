#!/bin/sh
# Local fake-cloud checks followed by isolated bootstrap and real local services.
set -eu
: "${MOMSO_S68_ARTIFACT_DIR:?set a fixture artifact directory}"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT HUP INT TERM
export MOMSO_S68_BOOTSTRAP_OUT="$scratch/installed"
python3 -m unittest discover -s scripts -p test_momso_release_manifest.py
python3 -m unittest discover -s scripts -p test_momso_release_sign.py
node --test scripts/test_momso_anywhere_google_end_to_end.mjs scripts/test_momso_anywhere_vm_startup.mjs scripts/test_momso_anywhere_cloud_run_revision.mjs
python3 -m unittest discover -s scripts -p test_momso_server_install.py
python3 -m unittest discover -s scripts -p test_momso_anywhere_install.py
python3 -m unittest discover -s apps/alpha/desktop/src-tauri/momso-server-update -p test_cloud_run_gateway.py
node --test scripts/test_momso_anywhere_destroy.mjs
