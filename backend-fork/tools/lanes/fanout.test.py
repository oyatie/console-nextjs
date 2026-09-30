#!/usr/bin/env python3
"""Offline contract tests for tools/lanes/fanout.py.

Run:  python3 tools/lanes/fanout.test.py

These are the rules a lane harness cannot get wrong without lying, and each
was wrong here:

  1. `_in_slice` decided membership with exact-set equality (`f not in allow`),
     so a lane declaring a DIRECTORY saw every file beneath it as an out-of-slice
     breach. A harness whose isolation report is false in the safe direction
     still trains you to ignore it.
  2. `_build_env` set RUSTC_WRAPPER=sccache without CARGO_INCREMENTAL=0. Per
     scripts/console/lane-env.sh:26-37 that makes the cache invisible rather than
     cold: 0 hits, 0 misses, 0 non-cacheable. The wrapper looked configured and
     bought nothing.
  3. `run` measured isolation and never asked whether the work should exist.
     A green or missing probe, a missing binary, and a probe that stays red
     after the implementer are the same class: occupancy is not a lane.
     `_codex` must not start unless the named probe is currently red.

No agent, no network -- this proves the LOGIC, not any lane's judgement.
"""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent


def _load():
    spec = importlib.util.spec_from_file_location("fanout_under_test", HERE / "fanout.py")
    module = importlib.util.module_from_spec(spec)
    # Registered before exec: @dataclass resolves annotations through
    # sys.modules[cls.__module__] and raises AttributeError without it.
    sys.modules["fanout_under_test"] = module
    spec.loader.exec_module(module)
    return module


FANOUT = _load()
FAILURES: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    print(f"{'PASS' if ok else 'FAIL'} {name}{'' if ok else f'  -- {detail}'}")
    if not ok:
        FAILURES.append(name)


def main() -> int:
    in_slice = FANOUT._in_slice

    with patch.object(
        FANOUT.subprocess, "run",
        return_value=SimpleNamespace(stdout=" M backend/app/src/lib.rs\n"),
    ):
        check(
            "porcelain paths retain the leading status column",
            FANOUT._changed(Path(".")) == ["backend/app/src/lib.rs"],
            "stripping git output first drops the first path character",
        )

    # --- slice membership is a PATH prefix rule, not string or set equality ---
    check(
        "a declared directory covers files beneath it",
        in_slice("backend/crates/payroll/domain/src/lib.rs", ["backend/crates/payroll"]),
        "this is the exact-set bug: a lane owning a directory reported every file in it as a breach",
    )
    check("a declared path matches itself", in_slice("a/b", ["a/b"]))
    check("a declared file matches exactly", in_slice("a/b.rs", ["a/b.rs"]))
    check(
        "a shared string prefix is NOT a shared directory",
        not in_slice("backend/crates/payrollx/src/lib.rs", ["backend/crates/payroll"]),
        "payrollx is not inside payroll",
    )
    check("a path outside every entry is out of slice", not in_slice("scripts/x.mjs", ["backend/crates/payroll"]))
    check(
        "an empty allow list denies everything",
        not in_slice("a/b.rs", []),
        "a guard that examines zero subjects must not pass everything",
    )
    check("a trailing slash is tolerated", in_slice("a/b.rs", ["a/"]))
    check("blank entries are ignored, not treated as a wildcard", not in_slice("a/b.rs", ["", "   "]))

    ok = False
    try:
        in_slice("a/b", ["../etc"])
    except ValueError:
        ok = True
    check(
        "'..' in an allow entry is refused, not normalised",
        ok,
        "collapsing ownership across directories is what a slice must not do",
    )

    # --- the lane build environment must make sccache visible ---
    env = FANOUT._build_env()
    if env.get("RUSTC_WRAPPER") == "sccache":
        check(
            "CARGO_INCREMENTAL is pinned to 0 whenever sccache is the wrapper",
            env.get("CARGO_INCREMENTAL") == "0",
            "incremental compilations are never cacheable; sccache never SEES them "
            "(lane-env.sh:26-37 measured 0 hits / 0 misses / 0 non-cacheable)",
        )
        check("the cache ceiling is set", bool(env.get("SCCACHE_CACHE_SIZE")))
    else:
        print("SKIP sccache env checks -- sccache is not installed on this machine")

    # --- a lane is a currently-red named command, not a free path ---
    class Proc:
        def __init__(self, code: int) -> None:
            self.returncode = code

    def runner_of(code: int):
        def run(_cmd, **_kwargs):
            return Proc(code)
        return run

    cwd = Path(".")
    missing = FANOUT.admit_lane({"lane": "1"}, cwd=cwd, runner=runner_of(1))
    check(
        "a spec with no probe is refused",
        missing is not None and "missing probe" in missing,
        "occupancy of a path is not a lane",
    )
    ran = []
    def run_if_called(_cmd, **_kwargs):
        ran.append(1)
        return Proc(1)
    blank = FANOUT.admit_lane(
        {"lane": "1", "probe": "   "}, cwd=cwd, runner=run_if_called
    )
    check(
        "a blank probe is refused without running anything",
        blank is not None and "missing probe" in blank and not ran,
    )
    green = FANOUT.admit_lane(
        {"lane": "1", "probe": "true"}, cwd=cwd, runner=runner_of(0)
    )
    check(
        "a GREEN probe on the clean tree is refused",
        green is not None and "GREEN" in green,
        "the lock already holds, or the probe cannot fail",
    )
    red = FANOUT.admit_lane(
        {"lane": "1", "probe": "false"}, cwd=cwd, runner=runner_of(1)
    )
    check("a currently-RED probe is admitted", red is None)
    missing_bin = FANOUT.admit_lane(
        {"lane": "1", "probe": "no-such-cmd"}, cwd=cwd, runner=runner_of(127)
    )
    check(
        "exit 127 is a missing binary, not a hole",
        missing_bin is not None and "not an executable command" in missing_bin,
    )
    refused = FANOUT.admit_spec(
        [
            {"lane": "1", "probe": "false"},
            {"lane": "2", "probe": "true"},
        ],
        cwd=cwd,
        runner=lambda cmd, **k: Proc(0 if "true" in str(cmd) else 1),
    )
    check(
        "one GREEN probe refuses the whole spec",
        len(refused) == 1 and "lane 2" in refused[0],
        "occupancy of the red lane cannot substitute for the green one",
    )

    # --- run() must not start an implementer unless every probe is red ---
    orig_repo = FANOUT.REPO
    orig_lanes = FANOUT.LANES_ROOT
    orig_codex = FANOUT._codex
    orig_changed = FANOUT._changed
    started: list[str] = []

    def fake_codex(prompt, cwd, sandbox, model, log):
        started.append(str(cwd))
        log.parent.mkdir(parents=True, exist_ok=True)
        log.write_text("")
        return True, "done", ""

    try:
        FANOUT._codex = fake_codex
        FANOUT._changed = lambda _wt: []
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            FANOUT.REPO = root
            FANOUT.LANES_ROOT = root / "lanes"
            (root / "lanes" / "lane-1").mkdir(parents=True)
            spec = root / "spec.json"

            spec.write_text(json.dumps([{
                "lane": "1",
                "prompt": "implement something",
                "allow": ["backend/crates/x"],
            }]))
            started.clear()
            rc = FANOUT.run_lanes(spec, "unused", "workspace-write",
                                  runner=runner_of(1))
            check(
                "run() does not start _codex when the spec has no probe",
                rc == 1 and not started,
            )

            spec.write_text(json.dumps([{
                "lane": "1",
                "probe": "true",
                "prompt": "implement something",
                "allow": ["backend/crates/x"],
            }]))
            started.clear()
            rc = FANOUT.run_lanes(spec, "unused", "workspace-write",
                                  runner=runner_of(0))
            check(
                "run() does not start _codex when the probe is already GREEN",
                rc == 1 and not started,
            )

            spec.write_text(json.dumps([{
                "lane": "1",
                "probe": "no-such-cmd",
                "prompt": "implement something",
                "allow": ["backend/crates/x"],
            }]))
            started.clear()
            rc = FANOUT.run_lanes(spec, "unused", "workspace-write",
                                  runner=runner_of(127))
            check(
                "run() does not start _codex for a missing binary",
                rc == 1 and not started,
            )

            (root / "lanes" / "lane-2").mkdir()
            spec.write_text(json.dumps([
                {
                    "lane": "1",
                    "probe": "false",
                    "prompt": "real hole",
                    "allow": ["backend/crates/x"],
                },
                {
                    "lane": "2",
                    "probe": "true",
                    "prompt": "already green",
                    "allow": ["backend/crates/y"],
                },
            ]))
            started.clear()
            rc = FANOUT.run_lanes(
                spec, "unused", "workspace-write",
                runner=lambda cmd, **k: Proc(0 if "true" in str(cmd) else 1),
            )
            check(
                "run() starts no implementer when any lane in the spec is GREEN",
                rc == 1 and not started,
                "a red sibling is occupancy, not permission to fan out the green one",
            )

            spec.write_text(json.dumps([{
                "lane": "1",
                "probe": "stale",
                "prompt": "already locked on trunk",
                "allow": ["backend/crates/x"],
            }]))
            started.clear()

            def green_on_tip(cmd, *, cwd, **_kwargs):
                # Integration tree already green; a stale worktree still red.
                return Proc(0 if Path(cwd) == FANOUT.REPO else 1)

            rc = FANOUT.run_lanes(spec, "unused", "workspace-write",
                                  runner=green_on_tip)
            check(
                "run() does not start _codex when the probe is GREEN on the integration tree",
                rc == 1 and not started,
                "a stale worktree looking red is not a hole",
            )

            # Admit red, implementer runs, probe stays red: occupancy is not success.
            n = {"calls": 0}

            def stay_red(_cmd, **_kwargs):
                n["calls"] += 1
                return Proc(1)

            spec.write_text(json.dumps([{
                "lane": "1",
                "probe": "false",
                "prompt": "implement something",
                "allow": ["backend/crates/x"],
            }]))
            started.clear()
            rc = FANOUT.run_lanes(spec, "unused", "workspace-write",
                                  runner=stay_red)
            check(
                "run() starts _codex when the probe is red, then fails if it stays red",
                rc == 1 and len(started) == 1 and n["calls"] >= 2,
            )

            # Admit red, implementer runs, same probe is now green: success.
            n["calls"] = 0

            def flip(_cmd, **_kwargs):
                n["calls"] += 1
                # admit on the integration tree, then on the worktree (both
                # must be red); later call is the close-check.
                return Proc(1 if n["calls"] <= 2 else 0)

            started.clear()
            rc = FANOUT.run_lanes(spec, "unused", "workspace-write",
                                  runner=flip)
            check(
                "run() succeeds only when isolation holds and the named probe is now green",
                rc == 0 and len(started) == 1,
            )
    finally:
        FANOUT.REPO = orig_repo
        FANOUT.LANES_ROOT = orig_lanes
        FANOUT._codex = orig_codex
        FANOUT._changed = orig_changed

    print()
    if FAILURES:
        print(f"{len(FAILURES)} FAILED: {', '.join(FAILURES)}")
        return 1
    print("ALL PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
