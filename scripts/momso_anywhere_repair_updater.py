"""Reinstall only the updater on an existing VM, without touching data or its journal."""
import argparse
import json
from pathlib import Path
import re

from momso_anywhere_update_install import refresh_worker
from momso_server_install import GoogleProvider, is_link_or_reparse, key_directory


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--account', required=True)
    parser.add_argument('--project', required=True)
    parser.add_argument('--state', type=Path, required=True)
    args = parser.parse_args()
    if not re.fullmatch(r'[A-Za-z0-9._%+@-]+', args.account) or not re.fullmatch(r'[a-z][a-z0-9-]{4,28}[a-z0-9]', args.project):
        raise SystemExit('계정 또는 프로젝트를 확인해 주세요.')
    state = json.loads(args.state.read_text(encoding='utf-8'))
    install_id = state.get('id', '')
    if (state.get('version') != 1 or state.get('account') != args.account
            or state.get('project') != args.project or not re.fullmatch(r'[a-f0-9]{12}', install_id)):
        raise SystemExit('설치 기록을 확인해 주세요.')
    key = key_directory(args.state, install_id, False) / 'ssh'
    if is_link_or_reparse(key) or not key.is_file():
        raise SystemExit('기존 설치의 연결 키를 찾지 못했습니다.')
    refresh_worker(GoogleProvider(args.account, args.project, install_id, key))
    print('작업자를 다시 시작했습니다. 복구와 상태를 확인해 주세요.')


if __name__ == '__main__':
    main()
