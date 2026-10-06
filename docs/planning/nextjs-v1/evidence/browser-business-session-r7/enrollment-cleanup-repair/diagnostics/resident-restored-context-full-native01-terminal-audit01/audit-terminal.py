#!/usr/bin/env python3
"""Artifact-only terminal audit; no product/test/source operations."""
from pathlib import Path
import hashlib
import json
import re

OUT = Path(__file__).resolve().parent
BASE = OUT.parent.parent
RUN = "resident-restored-context-full-native01"
PATHS = {"report": BASE / (RUN + ".json"), "log": BASE / (RUN + ".log"), "inputs": BASE / (RUN + ".inputs.json")}

def sha(data):
    return hashlib.sha256(data).hexdigest()

def require(condition, label):
    if not condition:
        raise ValueError(label)

def main():
    require(all(p.is_file() for p in PATHS.values()), "terminal report/log/maps not all available")
    require(not (OUT / "audit.json").exists(), "audit output already exists")
    raw = {k: p.read_bytes() for k, p in PATHS.items()}
    report = json.loads(raw["report"])
    inputs = json.loads(raw["inputs"])
    log = raw["log"].decode("utf8")
    require(report["id"] == RUN, "wrong report identity")
    require(report["log_sha256"] == sha(raw["log"]), "terminal log hash mismatch")
    require(report["inputs_sha256"] == sha(raw["inputs"]), "terminal maps hash mismatch")
    require(report["timed_out"] is False, "outer timeout must be separately audited")
    require(inputs["before"] == inputs["after"] and inputs["unchanged"] is True and report["inputs_unchanged"] is True,
            "captured before/after input drift")
    summaries = re.findall(r"test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out; finished in ([0-9.]+)s", log)
    require(len(summaries) == 1, "expected exactly one actual native terminal summary")
    result, passed, failed, ignored, measured, filtered, body_seconds = summaries[0]
    passed, failed, ignored, measured, filtered = map(int, (passed, failed, ignored, measured, filtered))
    announced = re.findall(r"^running (\d+) tests$", log, re.M)
    require(len(announced) == 1, "native announcement missing or ambiguous")
    names = re.findall(r"^test ([A-Za-z0-9_:]+) \.\.\.", log, re.M)
    require(len(names) == len(set(names)), "duplicate raw native test names")
    require(len(names) == passed + failed + ignored == int(announced[0]), "raw native name/summary counts disagree")
    require(report["counts"] == {"passed": passed, "failed": failed, "skipped": ignored, "filtered": filtered, "executed": passed + failed}, "wrapper/raw native counts disagree")
    failed_names = []
    if failed:
        require("\nfailures:\n" in log, "native failed-name summary absent")
        last = log.rsplit("\nfailures:\n", 1)[1].split("\ntest result:", 1)[0]
        failed_names = [line.strip() for line in last.splitlines() if line.strip()]
        require(len(failed_names) == failed and set(failed_names).issubset(names), "native failed-name summary inconsistent")
    payloads = re.findall(r"PRODUCT_BROWSER_SCENARIOS (\{[^\n]*\})", log)
    require(len(payloads) == 1, "exactly one real-browser terminal payload required")
    payload_raw = (payloads[0] + "\n").encode()
    browser = json.loads(payloads[0])
    require(browser["kind"] == "report" and browser["v"] == 1, "browser terminal identity mismatch")
    scenarios = browser["scenarios"]
    ids = [s["id"] for s in scenarios]
    require(len(ids) == len(set(ids)), "duplicate scenario IDs")
    require(ids == ["P" + str(i).zfill(2) for i in range(1, 18)], "scenario inventory differs from all17 fixed cases")
    statuses = {status: sum(s["status"] == status for s in scenarios) for status in ("passed", "failed", "skipped", "unreached")}
    require(statuses == browser["counts"], "browser scenario/raw counts disagree")
    require(browser["discovered"] == 17 and browser["executed"] == statuses["passed"] + statuses["failed"], "browser execution counts inconsistent")
    expected_pins = {"tools/test-resident-authenticator.mjs": "88cdfea200731ab32fcda0c797a96eee6ce49a42e6f174208fed29195105cd2e", "backend-fork/backend/app/tests/auth_rest/resident_authenticator.rs": "c2fc3e190d7b9933cdaf77958a8a9dc11a6ab8df83fa69362c7a42d5bc67412c", "tools/browser-business-restore.mjs": "183df5e840c6fbd46c77ca7fa4ba8301801606870b970173a89ac784a27ddf4c"}
    require(all(inputs["before"][k] == v for k, v in expected_pins.items()), "run not on declared frozen composite inputs")
    require(len(inputs["before"]) == len(inputs["after"]) == 2958, "input inventory count changed")
    added = [n for n in names if "resident_authenticator::" in n]
    expected_added = ["browser_sessions::resident_authenticator::failed_close_is_cached_without_restarting_cleanup_and_normal_drop_stays_failing", "browser_sessions::resident_authenticator::injected_reader_setup_failure_preserves_constructor_error_and_resource_custody", "browser_sessions::resident_authenticator::injected_stderr_setup_failure_preserves_constructor_error_and_resource_custody", "browser_sessions::resident_authenticator::injected_writer_setup_failure_preserves_constructor_error_and_resource_custody"]
    require(added == expected_added, "four constructor/cache cases changed or absent")
    native_all_green = passed == 125 and failed == ignored == measured == filtered == 0 and len(names) == 125
    browser_all_green = statuses == {"passed": 17, "failed": 0, "skipped": 0, "unreached": 0} and browser["executed"] == 17
    accepted = native_all_green and browser_all_green and report["exit_code"] == 0 and result == "ok"
    first_frame_label = "infrastructure: resident secure origin not ready"
    first_frame_observed = first_frame_label in log
    p16 = next(s for s in scenarios if s["id"] == "P16")
    queued = next((c for c in p16.get("checks", []) if c["id"] == "queued-react-css"), None)
    audit = {"schema": "resident-restored-context-full-native-terminal-audit/1", "verdict": "ACCEPT_TERMINAL_EVIDENCE; REQUIRED_ACCEPTANCE_" + ("PASSED" if accepted else "FAILED"), "scope": "Independent preserved terminal report/log/captured-map and embedded real-browser payload audit. No rerun, original root tool-session consumption, live source regeneration, process census, full product/release or GitHub approval.", "inputs": {k: {"path": str(p), "bytes": len(raw[k]), "sha256": sha(raw[k])} for k,p in PATHS.items()}, "native": {"announced": int(announced[0]), "unique_raw_test_names": len(names), "executed": passed + failed, "passed": passed, "failed": failed, "ignored": ignored, "measured": measured, "filtered": filtered, "failed_test_names": failed_names, "added_constructor_cache_tests": added, "exit_code": report["exit_code"], "outer_timeout": report["timed_out"], "body_duration_seconds": float(body_seconds), "wrapper_duration_seconds": report["duration_seconds"]}, "browser": {"discovered": browser["discovered"], "executed": browser["executed"], **statuses, "failed_scenarios": [s["id"] for s in scenarios if s["status"] == "failed"], "P16_status": p16["status"], "P16_checks": {c["id"]: c["status"] for c in p16.get("checks", [])}, "queued_react_css": queued, "payload_path": str(OUT / "browser-payload.json"), "payload_sha256": sha(payload_raw), "payload_provenance": "Exact embedded compact JSON bytes plus one newline; no regenerated browser run or current source substitution."}, "source_inputs": {"sha256": sha(raw["inputs"]), "before_count": len(inputs["before"]), "after_count": len(inputs["after"]), "all_before_after_entries_equal": True, "selected_pins": expected_pins, "limit": "Captured maps audited exactly; no independent regeneration and no acceptance for later diagnostic-source application."}, "first_frame_failure": {"fixed_error_label_observed": first_frame_observed, "fixed_error_label": first_frame_label if first_frame_observed else None, "meaning": "Generic mismatch of first validated frame against fixed ready/origin/secure fields; label does not establish secureDocument failure.", "raw_first_frame": "UNOBSERVED on this frozen source", "setup_checkpoint": "UNOBSERVED on this frozen source", "specific_cause": "UNPROVEN", "host_load_cause": "UNPROVEN", "link_to_historical_sdk_cleanup_failure": "NOT_INFERRED", "limit": "Source-reviewed diagnostic02 was not part of these captured frozen inputs; no retroactive diagnostic/repair claim."}, "fixed_final_native121_material_semantics": {"required_slot": "final_native121_browser17_gate_terminal_report_log_inputs_payload_and_independent_audit", "inherited_native": 121, "added_constructor_cache_native": 4, "required_actual_native": 125, "required_actual_browser": 17, "zero_ignored_filtered_skipped_unreached_required": True, "native_all_green": native_all_green, "browser_all_green": browser_all_green, "required_acceptance_passed": accepted, "limit": "Terminal evidence can be accepted as honest RED. RED does not satisfy final-green material or grant merge admission."}, "cleanup": {"database_lane_cleanup_line_observed": bool(re.search(r"^clean: [^\n]+ removed with its volume$", log, re.M)), "limit": "Printed lane cleanup is not independent browser/group or temporary-scope custody."}, "remaining_required": ([] if accepted else ["Repair or legitimately reproduce unresolved required failure without weakening original bounds or substituting provider success.", "Fresh final-source unfiltered125-native/all17-browser acceptance after any applied diagnostic/repair."]) + ["Current author-separated cumulative source and metadata acceptance, final source bindings, immutable-head hosted CI and protected merge queue."], "actions": {"tests_executed": 0, "tracked_source_writes": 0, "artifact_only_audit_write": True}}
    require(all(p.read_bytes() == raw[k] for k,p in PATHS.items()), "terminal inputs drifted during audit")
    with (OUT / "browser-payload.json").open("xb") as f:
        f.write(payload_raw)
    with (OUT / "audit.json").open("x") as f:
        json.dump(audit, f, indent=2)
        f.write("\n")
    print(json.dumps({"audit_path": str(OUT / "audit.json"), "audit_sha256": sha((OUT / "audit.json").read_bytes()), "verdict": audit["verdict"], "native": {"executed": passed + failed, "passed": passed, "failed": failed}, "browser": {"executed": browser["executed"], **statuses}}))

if __name__ == "__main__":
    main()
