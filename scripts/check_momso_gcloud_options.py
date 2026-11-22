"""Compare installer gcloud flags with local CLI help without running cloud operations."""
import ast
import concurrent.futures
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCES = [
    ROOT / 'scripts' / name for name in (
        'momso_server_install.py', 'momso_anywhere_install.py',
        'momso_anywhere_backup_provision.py', 'momso_anywhere_vm_services.py',
        'momso_wake_install.py',
    )
]
GATEWAY = ROOT / 'apps/alpha/desktop/src-tauri/momso-server-update/cloud_run_gateway.py'


def initial_text(node):
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    if isinstance(node, ast.JoinedStr) and node.values and isinstance(node.values[0], ast.Constant):
        return node.values[0].value
    return ''


def collect_sequence(nodes, prefix=()):
    command = list(prefix)
    flags = set()
    actions = {'list', 'describe', 'create', 'update', 'delete', 'deploy', 'ssh', 'scp',
               'rm', 'enable', 'access', 'start', 'stop', 'set-service-account', 'add',
               'add-iam-policy-binding', 'remove-iam-policy-binding', 'update-traffic'}
    for node in nodes:
        text = initial_text(node)
        if text.startswith('--'):
            flags.add(text.split('=', 1)[0])
        elif not command or (len(command) < 5 and isinstance(node, ast.Constant)
                             and isinstance(node.value, str) and re.fullmatch('[a-z][a-z-]*', text)):
            if not text or text.startswith('-'):
                break
            command.append(text)
            if text in actions:
                break
        else:
            break
    for node in nodes:
        text = initial_text(node)
        if text.startswith('--'):
            flags.add(text.split('=', 1)[0])
    return tuple(command), flags


def catalog():
    commands = {}
    for path in [*SOURCES, GATEWAY]:
        tree = ast.parse(path.read_text(encoding='utf-8'))
        for call in ast.walk(tree):
            if not isinstance(call, ast.Call):
                continue
            if isinstance(call.func, ast.Name) and call.func.id == 'bind_custom_role':
                command, flags = collect_sequence(call.args[1:])
                if command:
                    commands.setdefault(command, set()).update(flags)
                continue
            if not isinstance(call.func, ast.Attribute):
                continue
            method, owner = call.func.attr, call.func.value
            if not isinstance(owner, ast.Name) or owner.id not in {'p', 'provider', 'self'}:
                continue
            sequences = []
            if path == GATEWAY and method == '_run':
                sequences = [(call.args, ('run',))]
            elif path != GATEWAY and method == 'run':
                sequences = [(call.args, ())]
            elif path != GATEWAY and method == 'ensure':
                sequences = [(arg.elts, ()) for arg in call.args if isinstance(arg, (ast.List, ast.Tuple))]
            for nodes, prefix in sequences:
                command, flags = collect_sequence(nodes, prefix)
                if command and command[0] in {'alpha', 'compute', 'storage', 'kms', 'iam', 'run', 'secrets', 'iap', 'projects', 'services'}:
                    if len(command) == 1:
                        continue  # dynamic compute destroy commands are expanded below
                    commands.setdefault(command, set()).update(flags)
    # These command words and flags are assembled from variables in destroy and
    # gateway deployment paths, rather than appearing together in one call.
    for resource in (('instances',), ('routers',), ('firewall-rules',), ('networks', 'subnets'), ('networks',)):
        for action in ('list', 'delete'):
            command = ('compute', *resource, action)
            flags = {'--filter'} if action == 'list' else set()
            if resource == ('instances',): flags.add('--zones' if action == 'list' else '--zone')
            if resource in (('routers',), ('networks', 'subnets')): flags.add('--regions' if action == 'list' else '--region')
            commands.setdefault(command, set()).update(flags)
    commands.setdefault(('run', 'deploy'), set()).update({
        '--image', '--no-traffic', '--service-account', '--labels', '--network', '--subnet',
        '--vpc-egress', '--timeout', '--set-secrets', '--no-allow-unauthenticated', '--set-env-vars',
    })
    commands.setdefault(('run', 'services', 'update-traffic'), set()).add('--to-revisions')
    # GoogleProvider.run appends these wide flags to each invocation; parse=True
    # additionally appends --format=json.
    for flags in commands.values():
        flags.update({'--account', '--project', '--quiet', '--format'})
    return commands


def supports(help_text, flag):
    if re.search(re.escape(flag) + r'(?![a-z0-9-])', help_text):
        return True
    base = flag[5:] if flag.startswith('--no-') else flag[2:]
    return bool(re.search(re.escape('--[no-]' + base) + r'(?![a-z0-9-])', help_text))


def validate(command, flags, executable):
    env = dict(os.environ, CLOUDSDK_PAGER='', PAGER='', CLOUDSDK_CORE_DISABLE_PROMPTS='1')
    result = subprocess.run([*executable, *command, '--help'], capture_output=True, text=True,
                            encoding='utf-8', errors='replace', env=env, timeout=90,
                            creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
    if result.returncode:
        return command, ['help_failed']
    return command, sorted(flag for flag in flags if not supports(result.stdout, flag))


def main():
    executable = shutil.which('gcloud') or shutil.which('gcloud.cmd')
    if not executable:
        print('gcloud_not_installed', file=sys.stderr)
        return 1
    prefix = [executable]
    if os.name == 'nt' and executable.lower().endswith(('.cmd', '.bat')):
        root = Path(executable).resolve().parent.parent
        python = root / 'platform/bundledpython/python.exe'
        prefix = [str(python) if python.is_file() else sys.executable, '-S', str(root / 'lib/gcloud.py')]
    commands = catalog()
    problems = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        for command, missing in pool.map(lambda item: validate(*item, prefix), sorted(commands.items())):
            if missing:
                problems.append((command, missing))
    for command, missing in problems:
        print('gcloud ' + ' '.join(command) + ': ' + ', '.join(missing))
    print(f'commands={len(commands)} unsupported={len(problems)}')
    return bool(problems)


if __name__ == '__main__':
    raise SystemExit(main())
