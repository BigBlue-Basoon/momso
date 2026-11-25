#!/usr/bin/env python3
"""S147 rehearsal state machine. Live adapters are intentionally not connected."""
import argparse
import json
import re
import sys


class RehearsalFailure(Exception):
    def __init__(self, code):
        allowed = {'protected_installation', 'ownership_unverified', 'operation_failed', 'install_failed', 'marker_digest_invalid', 'backup_unverified', 'pause_unverified', 'restore_failed', 'digest_mismatch', 'cleanup_failed', 'inventory_unavailable', 'leftovers_unverified'}
        self.code = code if code in allowed else 'operation_failed'
        super().__init__(self.code)


def run_rehearsal(installation_id, protected_ids, adapter, emit=lambda *_: None):
    """Exercise a disposable installation. Adapter methods must prove ownership.

    An adapter may report a code, but exception text is never exposed. The
    caller must obtain the protected list from the project before this call.
    """
    if (not isinstance(installation_id, str) or not re.fullmatch(r'[a-f0-9]{12}', installation_id)
            or not isinstance(protected_ids, (set, list, tuple))
            or any(not isinstance(x, str) or not re.fullmatch(r'[a-f0-9]{12}(?:[a-f0-9]{12})?', x) for x in protected_ids)
            or any(x[:12] == installation_id for x in protected_ids)):
        raise RehearsalFailure('protected_installation')
    steps, leftovers = {}, {'status': 'unverified'}
    attempted = False
    result = {'outcome': 'failed', 'steps': steps, 'leftovers': leftovers}

    def step(name, action):
        try:
            value = action()
            steps[name] = {'status': 'ok', 'reason': None}
            emit(name, 'ok', None)
            return value
        except RehearsalFailure as exc:
            steps[name] = {'status': 'failed', 'reason': exc.code}
            emit(name, 'failed', exc.code)
            raise
        except Exception:
            steps[name] = {'status': 'failed', 'reason': 'operation_failed'}
            emit(name, 'failed', 'operation_failed')
            raise RehearsalFailure('operation_failed') from None

    def require(value, reason):
        if value is not True:
            raise RehearsalFailure(reason)
        return value

    def marker_digest():
        value = adapter.write_marker(installation_id)
        if not isinstance(value, str) or not re.fullmatch(r'[a-f0-9]{64}', value):
            raise RehearsalFailure('marker_digest_invalid')
        return value

    def paused_state():
        value = adapter.check_paused(installation_id)
        if value != {'vmAbsent': True, 'gatewayAbsent': True, 'backupPresent': True}:
            raise RehearsalFailure('pause_unverified')
        return value

    def checked_inventory():
        value = adapter.inventory(installation_id)
        if not isinstance(value, dict) or value.get('complete') is not True or value.get('remaining') != []:
            raise RehearsalFailure('leftovers_unverified')
        return value

    try:
        step('preflight', lambda: require(adapter.preflight(installation_id), 'ownership_unverified'))
        attempted = True
        step('install', lambda: require(adapter.install(installation_id), 'install_failed'))
        digest = step('write_marker', marker_digest)
        step('pause', lambda: require(adapter.pause(installation_id), 'backup_unverified'))
        step('check_paused', paused_state)
        step('reinstall', lambda: require(adapter.reinstall(installation_id), 'restore_failed'))
        step('read_marker', lambda: require(adapter.read_marker(installation_id) == digest, 'digest_mismatch'))
        result['outcome'] = 'ok'
    except RehearsalFailure as exc:
        result['reason'] = exc.code
    finally:
        if attempted:
            try:
                step('cleanup', lambda: require(adapter.cleanup(installation_id), 'cleanup_failed'))
            except RehearsalFailure:
                result['outcome'] = 'failed'
                result.setdefault('reason', 'cleanup_failed')
            try:
                inventory = step('inventory', checked_inventory)
                result['leftovers'] = inventory
            except RehearsalFailure as exc:
                result['outcome'] = 'failed'
                result.setdefault('reason', exc.code)
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description='시험 설치 쉬기와 복원 확인')
    for name in ('account', 'project', 'release-manifest-url', 'bundle', 'public-key', 'channel', 'google-client-id', 'owner-sub', 'login-origin'):
        parser.add_argument('--' + name, required=True)
    parser.add_argument('--protect-installation-id', action='append', default=[])
    parser.parse_args(argv)
    # Never provision a real VM until install-time restore and its pre-backup
    # fence are implemented and independently tested.
    result = {'outcome': 'failed', 'steps': {'preflight': {'status': 'failed', 'reason': 'restore_not_integrated'}}, 'leftovers': {'status': 'not_created'}}
    print('preflight: failed (restore_not_integrated)')
    print(json.dumps(result, ensure_ascii=False, separators=(',', ':')))
    return 1


if __name__ == '__main__':
    sys.exit(main())
