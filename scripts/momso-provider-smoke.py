#!/usr/bin/env python3
"""Small, explicit provider CLI smoke check; never prints CLI output or secrets."""
import argparse
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile


PROMPT = "Reply with exactly OK."


def gemini_key_from_windows_store():
    """Read the same generic credential as momso's Windows keyring store."""
    if os.name != "nt":
        return None
    class Credential(ctypes.Structure):
        _fields_ = [
            ("Flags", wintypes.DWORD), ("Type", wintypes.DWORD),
            ("TargetName", wintypes.LPWSTR), ("Comment", wintypes.LPWSTR),
            ("LastWritten", wintypes.FILETIME), ("CredentialBlobSize", wintypes.DWORD),
            ("CredentialBlob", ctypes.POINTER(ctypes.c_ubyte)),
            ("Persist", wintypes.DWORD), ("AttributeCount", wintypes.DWORD),
            ("Attributes", ctypes.c_void_p), ("TargetAlias", wintypes.LPWSTR),
            ("UserName", wintypes.LPWSTR),
        ]
    advapi = ctypes.WinDLL("advapi32", use_last_error=True)
    advapi.CredReadW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
                                ctypes.POINTER(ctypes.POINTER(Credential))]
    advapi.CredReadW.restype = wintypes.BOOL
    advapi.CredFree.argtypes = [ctypes.c_void_p]
    pointer = ctypes.POINTER(Credential)()
    if not advapi.CredReadW("keyring:ai:gemini:api-key@app.momso.desktop", 1, 0,
                            ctypes.byref(pointer)):
        return None
    try:
        value = pointer.contents
        return ctypes.string_at(value.CredentialBlob, value.CredentialBlobSize).decode("utf-16-le")
    except (UnicodeError, ValueError):
        return None
    finally:
        advapi.CredFree(pointer)


def invoke(command, *, env, cwd, timeout=120):
    try:
        result = subprocess.run(command, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                timeout=timeout, check=False)
    except FileNotFoundError:
        return None, b""
    except subprocess.TimeoutExpired:
        return "timeout", b""
    except OSError:
        return "start_failed", b""
    return result.returncode, result.stdout


def find_cli(name):
    if name == "agy" and os.name == "nt":
        path = Path(os.environ.get("LOCALAPPDATA", "")) / "agy" / "bin" / "agy.exe"
        if path.is_file():
            return str(path)
    found = shutil.which(name + ".cmd") or shutil.which(name)
    if not found or os.name != "nt" or not found.lower().endswith((".cmd", ".bat")):
        return found
    root = Path(found).parent / "node_modules"
    if name == "claude":
        candidates = [root / "@anthropic-ai" / "claude-code" / "bin" / "claude.exe"]
    elif name == "codex":
        arch = os.environ.get("PROCESSOR_ARCHITECTURE", "").lower()
        package, target = (("codex-win32-arm64", "aarch64-pc-windows-msvc")
                           if arch == "arm64" else
                           ("codex-win32-x64", "x86_64-pc-windows-msvc"))
        tail = Path("vendor") / target / "bin" / "codex.exe"
        candidates = [root / "@openai" / "codex" / "node_modules" / "@openai" / package / tail,
                      root / "@openai" / package / tail, root / "@openai" / "codex" / tail]
    else:
        candidates = []
    return next((str(path) for path in candidates if path.is_file()), None)


def build_resources_available():
    if shutil.disk_usage(Path(__file__).resolve().parent).free < 60 * 1024 ** 3:
        return False
    if os.name != "nt":
        return False
    class MemoryStatus(ctypes.Structure):
        _fields_ = [("length", wintypes.DWORD), ("load", wintypes.DWORD),
                    ("total_phys", ctypes.c_uint64), ("avail_phys", ctypes.c_uint64),
                    ("total_page", ctypes.c_uint64), ("avail_page", ctypes.c_uint64),
                    ("total_virt", ctypes.c_uint64), ("avail_virt", ctypes.c_uint64),
                    ("avail_extended", ctypes.c_uint64)]
    state = MemoryStatus()
    state.length = ctypes.sizeof(state)
    return bool(ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(state))) and (
        state.avail_phys >= 4 * 1024 ** 3)


def check(provider, cli, *, base_env, invoke_fn=invoke, key_reader=gemini_key_from_windows_store,
          workspace, engine=False, resource_check=build_resources_available,
          cargo_finder=lambda: shutil.which("cargo")):
    if not cli:
        return "skipped", "cli_missing"
    env = base_env.copy()
    for name in ("GEMINI_API_KEY", "GOOGLE_API_KEY", "ANTHROPIC_API_KEY",
                 "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN", "OPENAI_API_KEY",
                 "CODEX_API_KEY", "MOMSO_CODEX_API_KEY"):
        env.pop(name, None)
    if provider == "codex":
        code, _ = invoke_fn([cli, "login", "status"], env=env, cwd=workspace)
        if code != 0:
            return "skipped", "login_unavailable"
        args = [cli, "exec", "--ephemeral", "--skip-git-repo-check",
                "--sandbox", "read-only", "-c", 'model_reasoning_effort="low"', PROMPT]
    elif provider == "claude":
        code, output = invoke_fn([cli, "auth", "status", "--json"], env=env, cwd=workspace)
        try:
            logged_in = code == 0 and json.loads(output).get("loggedIn") is True
        except (ValueError, AttributeError):
            logged_in = False
        if not logged_in:
            return "skipped", "login_unavailable"
        args = [cli, "-p", PROMPT, "--model", "haiku", "--effort", "low",
                "--no-session-persistence", "--tools", ""]
    else:
        try:
            key = key_reader()
        except OSError:
            return "skipped", "credential_store_unavailable"
        if not key:
            return "skipped", "api_key_missing"
        env["GEMINI_API_KEY"] = key
        # The direct CLI check gets isolated files; the engine test needs the
        # existing toolchain and uses the backend's own temporary Gemini dir.
        if not engine:
            env["HOME"] = str(workspace)
            env["USERPROFILE"] = str(workspace)
            env["APPDATA"] = str(workspace)
            env["LOCALAPPDATA"] = str(workspace)
        args = [cli, "--settings", '{"modelProvider":"gemini"}',
                "--model", "gemini-2.5-flash-lite", "--effort", "low", "-p", PROMPT]
    if engine:
        if not resource_check():
            return "skipped", "build_resources_unavailable"
        cargo = cargo_finder()
        if not cargo:
            return "skipped", "cargo_missing"
        repo = Path(__file__).resolve().parent.parent
        env.pop("MOMSO_TEST_FAKE_CREDENTIAL_STORE", None)
        env.pop("MOMSO_ENGINE_FAKE", None)
        env.pop("GEMINI_API_KEY", None)
        env.update({"MOMSO_RUN_LIVE_PROVIDER_SMOKE": "1", "CARGO_PROFILE_DEV_DEBUG": "0",
                    "CARGO_PROFILE_TEST_DEBUG": "0", "CARGO_INCREMENTAL": "0",
                    "CARGO_BUILD_JOBS": "1", "CARGO_TARGET_DIR": str(repo / "target")})
        args = [cargo, "test", "--manifest-path",
                str(repo / "apps/alpha/desktop/src-tauri/Cargo.toml"), "-j", "1",
                f"provider_engine_smoke::{provider}_one_turn", "--",
                "--exact", "--ignored", "--nocapture", "--test-threads=1"]
        code, output = invoke_fn(args, env=env, cwd=repo, timeout=None)
        if b"SKIPPED=api_key_missing" in output:
            return "skipped", "api_key_missing"
        if code == 0 and b"test result: ok. 1 passed" in output:
            found = []
            for key in ("MODEL_REPORTED", "EFFORT_REPORTED", "EFFORT_REQUESTED", "CONTEXT_REPORTED"):
                match = re.search((key + r"=(true|false)").encode("ascii"), output)
                if not match:
                    return "failed", "missing_engine_report"
                found.append(key.lower() + "=" + match.group(1).decode("ascii"))
            return "ok", ";".join(found)
        match = re.search(rb"SMOKE_THREAD_NOT_ARCHIVED=([0-9a-fA-F-]{36})", output)
        if match:
            return "failed", "thread_not_archived:" + match.group(1).decode("ascii")
        return "failed", "engine_failed" if isinstance(code, int) else str(code)
    code, output = invoke_fn(args, env=env, cwd=workspace)
    if code == 0 and output.strip().upper() == b"OK":
        return "ok", "-"
    if code is None:
        return "failed", "cli_missing"
    if isinstance(code, str):
        return "failed", code
    if code == 0:
        return "failed", "unexpected_reply" if output.strip() else "empty_reply"
    return "failed", f"cli_exit_{code}"


def self_test():
    calls = []
    fake_cli = (
        "import json,sys; a=sys.argv[1:]; "
        "print(json.dumps({'loggedIn':True}) if a[:3]==['auth','status','--json'] "
        "else 'OK')"
    )
    def fake(command, *, env, cwd, timeout=120):
        calls.append(command)
        if len(command) > 1 and command[1] == "test":
            assert env["MOMSO_RUN_LIVE_PROVIDER_SMOKE"] == "1"
            assert "GEMINI_API_KEY" not in env
            assert "fake-secret" not in " ".join(command)
            return 0, (b"MODEL_REPORTED=true\nEFFORT_REPORTED=true\nEFFORT_REQUESTED=true\n"
                       b"CONTEXT_REPORTED=true\ntest result: ok. 1 passed")
        if "--settings" in command:
            assert env.get("GEMINI_API_KEY") == "fake-secret"
        return invoke([sys.executable, "-c", fake_cli, *command[1:]],
                      env=env, cwd=cwd, timeout=timeout)
    with tempfile.TemporaryDirectory(prefix="momso-smoke-test-") as directory:
        workspace = Path(directory)
        for name in ("codex", "claude", "gemini"):
            result = check(name, "fake-cli", base_env={}, invoke_fn=fake,
                           key_reader=lambda: "fake-secret", workspace=workspace)
            assert result == ("ok", "-")
        assert len(calls) == 5
        assert sum(PROMPT in command for command in calls) == 3
        calls.clear()
        result = check("gemini", "fake-cli", base_env={}, invoke_fn=fake,
                       key_reader=lambda: None, workspace=workspace)
        assert result == ("skipped", "api_key_missing") and not calls
        def denied(command, *, env, cwd, timeout=120):
            return 1, b"fake-secret"
        assert check("codex", "fake-cli", base_env={}, invoke_fn=denied, workspace=workspace) == (
            "skipped", "login_unavailable")
        def wrong_reply(command, *, env, cwd, timeout=120):
            return 0, b"not the requested answer"
        assert check("gemini", "fake-cli", base_env={}, invoke_fn=wrong_reply,
                     key_reader=lambda: "fake-secret", workspace=workspace) == (
                         "failed", "unexpected_reply")
        for name in ("codex", "claude", "gemini"):
            assert check(name, "fake-cli", base_env={}, invoke_fn=fake,
                         key_reader=lambda: "fake-secret", workspace=workspace,
                         engine=True, resource_check=lambda: True,
                         cargo_finder=lambda: "fake-cargo") == (
                             "ok", "model_reported=true;effort_reported=true;effort_requested=true;context_reported=true")
    print("fake checks: 9 passed, 0 failed")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--provider", choices=("codex", "claude", "gemini", "all"))
    parser.add_argument("--engine", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        if args.provider or args.engine:
            parser.error("--self-test cannot be combined with --provider or --engine")
        self_test()
        return
    if not args.provider:
        parser.error("--provider is required")
    providers = ("codex", "claude", "gemini") if args.provider == "all" else (args.provider,)
    print("Planned messages: " + ", ".join(f"{name}=1" for name in providers), flush=True)
    results = []
    with tempfile.TemporaryDirectory(prefix="momso-provider-smoke-") as directory:
        for name in providers:
            cli = find_cli("agy" if name == "gemini" else name)
            state, reason = check(name, cli, base_env=os.environ, workspace=Path(directory),
                                  engine=args.engine)
            results.append((name, state, reason))
    print("| provider | state | reason |")
    print("|---|---|---|")
    for name, state, reason in results:
        print(f"| {name} | {state} | {reason} |")
    if any(state != "ok" for _, state, _ in results):
        sys.exit(1)


if __name__ == "__main__":
    main()
