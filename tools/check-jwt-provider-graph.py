#!/usr/bin/env python3
"""Fail closed if a shipped or tested Cargo closure retains the old JWT provider."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import selectors
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
BACKEND = ROOT / "backend-fork/backend"
TARGETS = ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "aarch64-apple-darwin")
OWNERS = {"console-app", "console-platform-auth", "console-platform-push"}
TIMEOUT_SECONDS = 120
MAX_OUTPUT_BYTES = 8 * 1024 * 1024
PACKAGE = re.compile(r"^([\w-]+) v(\d+\.\d+\.\d+(?:-[\w.-]+)?(?:\+[\w.-]+)?)(?: \([^\n]*\))?$")
FEATURES = re.compile(r"^(?:[\w+./-]+(?:,[\w+./-]+)*)?$")


def inspect_graph(text):
    packages = {}
    errors = []
    for line in text.splitlines():
        if not line:
            continue  # Cargo separates workspace roots with blank lines.
        line = line.removesuffix(" (*)")
        identity, separator, features = line.partition("|")
        match = PACKAGE.fullmatch(identity)
        if not separator or not match or not FEATURES.fullmatch(features):
            errors.append("malformed package row")
            continue
        name, version = match.groups()
        value = (version, frozenset(features.split(",")) if features else frozenset())
        if name == "jsonwebtoken" and name in packages and packages[name] != value:
            errors.append("conflicting JWT version or features")
        packages[name] = value
        if name == "rsa":
            errors.append("vulnerable rsa crate is selected")
    if not OWNERS <= packages.keys():
        errors.append("required application/worker owners missing")
    jwt = packages.get("jsonwebtoken")
    if jwt is None or jwt[0] != "10.4.0":
        errors.append("expected JWT version missing")
    elif not {"aws_lc_rs", "use_pem"} <= jwt[1] or {"rust_crypto", "rsa"} & jwt[1]:
        errors.append("JWT must select AWS-LC and PEM without RustCrypto/RSA")
    return sorted(set(errors))


def bounded_command(command):
    # Cargo may spawn rustc for metadata; kill only this command's owned group.
    environment = {name: os.environ[name] for name in (
        "PATH", "HOME", "USER", "TMPDIR", "CARGO_HOME", "RUSTUP_HOME", "CARGO_TARGET_DIR",
        "SSL_CERT_FILE", "SSL_CERT_DIR") if name in os.environ}
    process = subprocess.Popen(command, cwd=BACKEND, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, start_new_session=True, env=environment)
    output = {"stdout": bytearray(), "stderr": bytearray()}
    deadline = time.monotonic() + TIMEOUT_SECONDS
    failure = None
    try:
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ, "stdout")
            selector.register(process.stderr, selectors.EVENT_READ, "stderr")
            while selector.get_map():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError("Cargo metadata deadline exceeded")
                for key, _ in selector.select(remaining):
                    chunk = os.read(key.fd, 65536)
                    if not chunk:
                        selector.unregister(key.fileobj)
                        continue
                    output[key.data].extend(chunk)
                    if sum(map(len, output.values())) > MAX_OUTPUT_BYTES:
                        raise ValueError("Cargo metadata output budget exceeded")
        code = process.wait(timeout=max(0.01, deadline - time.monotonic()))
    except BaseException as error:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            if process.poll() is None:
                raise  # Never hide inability to fence a live owned process.
        except PermissionError:
            process.poll()  # Reap a terminal leader; this does not prove group fencing.
            raise
        code = process.wait()
        if not isinstance(error, (TimeoutError, ValueError, OSError, subprocess.TimeoutExpired)):
            raise
        failure = str(error)
    finally:
        process.stdout.close()
        process.stderr.close()
    return code, bytes(output["stdout"]), bytes(output["stderr"]), failure


def graph_inputs():
    paths = [BACKEND / "Cargo.lock", *sorted((ROOT / "backend-fork").rglob("Cargo.toml"))]
    return {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in paths}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true")
    options = parser.parse_args()
    before = graph_inputs()
    rows = []
    for target in TARGETS:
        for variant in ("default", "tested-union"):
            command = ["cargo", "+1.98.1", "tree", "--locked", "--workspace",
                       "--edges", "normal,build,dev", "--prefix", "none",
                       "--format", "{p}|{f}", "--color", "never", "--target", target]
            if options.offline:
                command.append("--offline")
            if variant == "tested-union":
                command += ["--features", "console-app/frontend-e2e,console-platform-auth-rest/dev-auth"]
            try:
                code, stdout, stderr, failure = bounded_command(command)
                errors = inspect_graph(stdout.decode("utf-8")) if code == 0 and not failure else [
                    failure or "Cargo metadata command failed"]
                row = {"id": target + "/" + variant, "command": command, "exit_code": code,
                       "errors": errors, "stdout_sha256": hashlib.sha256(stdout).hexdigest(),
                       "stderr_sha256": hashlib.sha256(stderr).hexdigest(),
                       "stdout": stdout.decode("utf-8", errors="replace"),
                       "stderr": stderr.decode("utf-8", errors="replace")}
            except (OSError, UnicodeError) as error:
                row = {"id": target + "/" + variant, "command": command,
                       "errors": [str(error)], "exit_code": None}
            rows.append(row)
    unchanged = before == graph_inputs()
    passed = len(rows) == 6 and unchanged and all(not row["errors"] for row in rows)
    print(json.dumps({"status": "passed" if passed else "failed", "inputs_unchanged": unchanged,
                      "input_hashes": before, "rows": rows}, indent=2))
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
