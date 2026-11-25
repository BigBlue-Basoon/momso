"""Run S173 mutations only in a disposable source copy; never contact Google."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parent.parent


def main():
    with tempfile.TemporaryDirectory(prefix="momso-s173-mutations-") as directory:
        root = Path(directory)
        shutil.copytree(ROOT / "scripts", root / "scripts", ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
        updater = Path("apps/alpha/desktop/src-tauri/momso-server-update")
        shutil.copytree(ROOT / updater, root / updater, ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
        env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
        cases = [
            ("proxy imports", "momso_anywhere_proxy_files.json", "json",
             "test_momso_anywhere_proxy_files.ProxyFilesTests.test_proxy_local_import_closure_is_in_manifest"),
            ("proxy health", "momso_anywhere_install.py", ("                health_check()", "                None"),
             "test_momso_anywhere_install.Tests.test_failed_private_health_probe_restores_old_revision"),
            ("wake before SSH", "momso_anywhere_pc_vm.py",
             ("                provider.run('compute', 'instances', action, provider.name, f'--zone={ZONE}', timeout=90)", "                None"),
             "test_momso_anywhere_pc_vm.Tests.test_stopped_vm_starts_before_ssh"),
            ("deletion resumes", "momso_anywhere_google_browser.mjs",
             ("remaining.push(item.key); networkPending = true; onProgress({ step: item.key, state: 'pending' });", "throw error;"),
             "node"),
            ("versioned updater imports", "../apps/alpha/desktop/src-tauri/momso-server-update/managed_helpers.py",
             ('"managed_helpers.py", "trust_rotation.py", "momso_managed_files.py"', '"managed_helpers.py", "momso_managed_files.py"'),
             "test_momso_anywhere_proxy_files.ProxyFilesTests.test_versioned_updater_includes_its_local_imports"),
            ("mobile browser provision", "momso_anywhere_vm_startup.mjs",
             ("if not any(shutil.which(name) for name in ('google-chrome','google-chrome-stable','chromium','chromium-browser')):missing.append('chromium')", "pass"),
             "node-browser"),
        ]
        failed = False
        for label, name, mutation, test in cases:
            path = root / "scripts" / name
            original = path.read_text(encoding="utf-8")
            if mutation == "json":
                values = json.loads(original)
                values.remove("momso_anywhere_pause_guard.py")
                changed = json.dumps(values)
            else:
                before, after = mutation
                if original.count(before) != 1:
                    raise RuntimeError(f"mutation target changed: {label}")
                changed = original.replace(before, after, 1)
            if test == "node":
                command = ["node", "--test", "--test-name-pattern=explicit discard survives authorization", "test_momso_anywhere_destroy.mjs"]
            elif test == "node-browser":
                command = ["node", "--test", "--test-name-pattern=VM startup installs Chromium only", "test_momso_anywhere_vm_startup.mjs"]
            else:
                command = [sys.executable, "-B", "-m", "unittest", "-q", test]
            clean = subprocess.run(command, cwd=root / "scripts", env=env, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=120)
            path.write_text(changed, encoding="utf-8", newline="\n")
            mutant = subprocess.run(command, cwd=root / "scripts", env=env, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=120)
            path.write_text(original, encoding="utf-8", newline="\n")
            detected = clean.returncode == 0 and mutant.returncode != 0
            print(f"{label}: original={clean.returncode}, mutant={mutant.returncode}, detected={detected}", flush=True)
            if not detected:
                print("original stderr:", clean.stderr[-1200:], "mutant stderr:", mutant.stderr[-1200:], flush=True)
                failed = True
        # This changes the fake provider's failure location, not product code.
        # It exposes whether an unrelated router error still allows gateway cleanup.
        fixture = root / "scripts/test_momso_anywhere_destroy.mjs"
        source = fixture.read_text(encoding="utf-8")
        source = source.replace("denyPolicy = false } = {})", "denyPolicy = false, rejectRouter = false } = {})", 1)
        source = source.replace("    deleted.push(path);", "    if (rejectRouter && path.includes('/routers/')) return ok({}, 403);\n    deleted.push(path);", 1)
        source += "\ntest('S173 router failure leaves later resources', async () => { const f = fixture({rejectRouter:true}); await assert.rejects(f.provider.destroy(id, () => {}, {discardData:true})); assert(f.deleted.some(p => p.includes('/instances/'))); assert(f.resources.has(`https://run.googleapis.com/v2/projects/${projectId}/locations/asia-northeast3/services/momso-any-${short}`)); assert(!f.deleted.some(p => p.includes('run.googleapis.com'))); });\n"
        fixture.write_text(source, encoding="utf-8", newline="\n")
        observed = subprocess.run(["node", "--test", "--test-name-pattern=S173 router failure", fixture.name],
                                  cwd=root / "scripts", env=env, capture_output=True, text=True,
                                  encoding="utf-8", errors="replace", timeout=120)
        print(f"router failure leaves gateway: observed={observed.returncode == 0}", flush=True)
        if observed.returncode:
            print(observed.stdout[-1200:], observed.stderr[-1200:], flush=True)
            failed = True
        return int(failed)


if __name__ == "__main__":
    raise SystemExit(main())
