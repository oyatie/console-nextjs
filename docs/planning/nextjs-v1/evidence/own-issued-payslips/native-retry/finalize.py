#!/usr/bin/env python3
"""Supplement the unchanged original report with positive cleanup/count custody."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import sys

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('evidence', type=Path)
args = parser.parse_args()
base = args.evidence.resolve(strict=True)
root = base.parents[1]
original_path = base / 'report.json'
original = json.loads(original_path.read_text())
provenance = json.loads((base / 'runner-provenance.json').read_text())
temporary = Path(provenance['owned_temporary_directory'])
if temporary.parent != Path('/private/tmp') or not temporary.name.startswith('own-payslip-pg-'):
    parser.error('unexpected temporary directory custody')
try:
    os.kill(provenance['observed_postmaster_pid'], 0)
    process_absent = False
except ProcessLookupError:
    process_absent = True
except PermissionError:
    process_absent = False

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

checks = {}
checks['original_report_passed'] = original['status'] == 'passed'
checks['runner_provenance_exact'] = sha(base / provenance['runner_source']) == provenance['runner_sha256']
checks['pg_ctl_stop_confirmed'] = original.get('cluster_stopped') is True and any(command['name'] == 'stop' and command['exit_code'] == 0 for command in original['commands'])
checks['owned_temporary_directory_absent'] = not temporary.exists() and not temporary.is_symlink()
checks['observed_postmaster_pid_absent'] = process_absent
checks['source_unchanged_during_run'] = original.get('source_inputs_unchanged') is True
checks['source_still_matches'] = all((root / name).is_file() and sha(root / name) == digest for name, digest in original['source_inputs'].items())
checks['worker_unchanged_during_run'] = original.get('worker_binary_unchanged') is True
checks['worker_still_matches'] = sha(Path(original['worker_binary']['path'])) == original['worker_binary']['sha256']
expected = {'worker-prerequisites': 2, 'auth-rest-full': 127, 'openapi-drift': 20}
suites = original['suites']
checks['exact_suite_population'] = len(suites) == len(expected) and {suite['name'] for suite in suites} == set(expected) and all(suite['discovered'] == suite['executed'] == suite['passed'] == expected[suite['name']] and suite['failed'] == suite['skipped'] == 0 and suite['status'] == 'passed' for suite in suites)
browser = original.get('browser_scenarios')
checks['all_browser_scenarios_passed'] = bool(browser and browser['discovered'] == browser['executed'] == 17 and browser['counts'] == {'passed': 17, 'failed': 0, 'unreached': 0, 'skipped': 0} and len(browser['scenarios']) == 17 and {scenario['id'] for scenario in browser['scenarios']} == {f'P{i:02}' for i in range(1, 18)} and all(scenario['status'] == 'passed' for scenario in browser['scenarios']))
receipt = {'status': 'passed' if all(checks.values()) else 'failed', 'scope': original['scope'], 'observed_at': datetime.now(timezone.utc).isoformat(), 'original_report_sha256': sha(original_path), 'original_runner_sha256': provenance['runner_sha256'], 'finalizer_sha256': sha(Path(__file__)), 'checks': checks, 'temporary_credential_files_removed': checks['owned_temporary_directory_absent'], 'unsupported_original_claim': 'ephemeral_credentials_destroyed is not credited; temporary-file removal does not claim process-memory or cryptographic erasure', 'suites': suites, 'browser_counts': browser['counts'] if browser else None}
(base / 'verified-report.json').write_text(json.dumps(receipt, indent=2) + '\n')
print(json.dumps(receipt, indent=2))
sys.exit(receipt['status'] != 'passed')
