#!/bin/sh
set -eu
repo=$(pwd)
url=https://github.com/BigBlue-Basoon/momso/releases/download/anywhere-dev-2026-11-25-8/bundle.tar.gz
curl -fsSL "$url" -o seq8.tar.gz
printf '%s  %s\n' 7e5502b3a8d2f2eacedc0dd1d79f112a6d64dc4d0d83d5255e2dfd7f455bafc5 seq8.tar.gz | sha256sum -c -
mkdir seq8
tar -xzf seq8.tar.gz -C seq8 momso-server idle-stop.py
printf '%s  %s\n' 707e5d952a6aa76652e22e2730e154278816c32f36ba39334e9f7abe79737bd8 seq8/momso-server | sha256sum -c -
fixture=$(sudo mktemp -d /var/lib/momso-s174.XXXXXX)
cleanup() { sudo rm -f -- "$fixture/agent.py"; sudo rmdir -- "$fixture"; }
trap cleanup EXIT
sudo chmod 755 "$fixture"
sudo cp apps/alpha/desktop/src-tauri/momso-idle-stop/fixture_backup_s174.py "$fixture/agent.py"
sudo chmod 644 "$fixture/agent.py"
export MOMSO_S174_SERVER="$repo/seq8/momso-server" MOMSO_S174_AGENT="$fixture/agent.py" PYTHONDONTWRITEBYTECODE=1
if [ "${1:-old}" = new ]; then
    export MOMSO_S174_IDLE="$repo/apps/alpha/desktop/src-tauri/momso-idle-stop/idle_stop.py"
else
    export MOMSO_S174_IDLE="$repo/seq8/idle-stop.py" MOMSO_S174_EXPECT_OLD=1
fi
cd apps/alpha/desktop/src-tauri/momso-idle-stop
python3 -m unittest -v test_real_server_s174
