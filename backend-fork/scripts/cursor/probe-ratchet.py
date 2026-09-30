#!/usr/bin/env python3
"""Hostile probes for Cursor ratchet hooks — banned substrings built at runtime."""
import json, os, subprocess, sys, tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
os.chdir(ROOT)


def resolve_hub(start: str) -> str:
    """If cwd is a linked worktree under <hub>/.worktrees/<name>, return <hub>."""
    parts = start.split(os.sep)
    if ".worktrees" in parts:
        idx = parts.index(".worktrees")
        hub = os.sep.join(parts[:idx])
        return hub if hub else os.sep
    return start


def run_hook(script, payload):
    p = subprocess.run(
        ["bash", script],
        input=json.dumps(payload).encode(),
        capture_output=True,
    )
    try:
        return json.loads(p.stdout.decode() or "{}")
    except Exception:
        return {"permission": "error", "raw": p.stdout.decode(), "err": p.stderr.decode()}

restack_cmd = " ".join(["git", "rebase", "origin/main"])

hub = resolve_hub(ROOT)
# Sibling of the hub (outside workspace), not a false path under hub/.worktrees/
wt_ok = f"git -C {hub} worktree add {hub}/.worktrees/lane-probe -b lane/probe origin/main"
wt_rel = "git worktree add .worktrees/lane-probe -b lane/probe origin/main"
wt_sibling = f"git -C {hub} worktree add {os.path.dirname(hub)}/console-lane-probe -b lane/probe origin/main"
wt_remove = f"git worktree remove {hub}/.worktrees/lane-probe"

tests = [
    ("reset", ".cursor/hooks/git-lock-enforcer.sh", {"command": "git reset --hard"}, "deny"),
    ("restack", ".cursor/hooks/git-lock-enforcer.sh", {"command": restack_cmd}, "allow"),
    ("merge", ".cursor/hooks/git-lock-enforcer.sh", {"command": "git merge other"}, "deny"),
    ("status", ".cursor/hooks/git-lock-enforcer.sh", {"command": "git status"}, "allow"),
    ("wt_inrepo_abs", ".cursor/hooks/git-lock-enforcer.sh", {"command": wt_ok}, "allow"),
    ("wt_inrepo_rel", ".cursor/hooks/git-lock-enforcer.sh", {"command": wt_rel}, "allow"),
    ("wt_sibling_deny", ".cursor/hooks/git-lock-enforcer.sh", {"command": wt_sibling}, "deny"),
    ("wt_remove_deny", ".cursor/hooks/git-lock-enforcer.sh", {"command": wt_remove}, "deny"),
    ("wfonly", ".cursor/hooks/cargo-scope-enforcer.sh", {"command": "cargo test --workflow-only -p x"}, "deny"),
]

failed = 0
for name, script, payload, expect in tests:
    out = run_hook(script, payload)
    perm = out.get("permission")
    ok = perm == expect
    if not ok:
        failed += 1
    print(f"{name}: got={perm} expect={expect} {'OK' if ok else 'FAIL ' + json.dumps(out)}")

bad = {
    "status": "done",
    "summary": "x",
    "filesChanged": ["a"],
    "redBaseline": "x",
    "verification": "x",
    "contractBreaches": "none",
    "enforcementPlacement": "n/a - adds no enforcement",
    "peripheralsUpdated": "n/a - nothing described this behaviour",
    "commands": [""],
    "headSha": "abc",
}
with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
    json.dump(bad, f)
    bad_path = f.name
p = subprocess.run(["node", "scripts/cursor/validate-lane-receipt.mjs", bad_path], capture_output=True)
print(f"empty_commands_validator_exit={p.returncode} (expect 1)")
if p.returncode != 1:
    failed += 1

# Prefer this lane's receipt; fall back to older ratchet receipt if present.
receipt_candidates = [
    ".cursor/receipts/cursor-worktree-layout.json",
    ".cursor/receipts/cursor-swarm-ratchet.json",
]
receipt = next((c for c in receipt_candidates if os.path.isfile(c)), None)
if receipt is None:
    print("build_receipt_exit=missing (expect a schema-valid process receipt)")
    failed += 1
else:
    p = subprocess.run(
        ["node", "scripts/cursor/validate-lane-receipt.mjs", receipt],
        capture_output=True,
    )
    print(f"build_receipt_exit={p.returncode} (expect 0) path={receipt}")
    if p.returncode != 0:
        failed += 1
        print(p.stderr.decode())

sys.exit(1 if failed else 0)
