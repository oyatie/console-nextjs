#!/usr/bin/env python3
"""Disposable password-auth PostgreSQL18 runner for already-compiled tests."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import socket as socket_api
import threading
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlencode

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--root', type=Path, required=True)
parser.add_argument('--suite', action='append', default=[], help='name=absolute compiled test path')
parser.add_argument('--filter', action='append', default=[], help='name=literal test filter')
args = parser.parse_args()
root = args.root.resolve(strict=True)
pg = Path('/opt/homebrew/opt/postgresql@18/bin')
node = Path('/Users/jasonlee/.local/share/fnm/node-versions/v24.21.0/installation/bin')
suites = [(name, Path(file).resolve(strict=True)) for name, file in (item.split('=', 1) for item in args.suite)]
filters = dict(item.split('=', 1) for item in args.filter)
if not suites:
    parser.error('at least one --suite required')
evidence = root / '.artifacts' / datetime.now(timezone.utc).strftime('own-issued-payslips-native-%Y%m%dT%H%M%S.%fZ')
evidence.mkdir(parents=True)
passwords = {key: secrets.token_hex(32) for key in ('POSTGRES_ADMIN_PASSWORD', 'CONSOLE_APP_POSTGRES_PASSWORD', 'CONSOLE_RT_POSTGRES_PASSWORD', 'CONSOLE_LEAVE_COMMAND_POSTGRES_PASSWORD', 'CONSOLE_ONTOLOGY_COMMAND_POSTGRES_PASSWORD', 'CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD')}
env = {key: value for key, value in os.environ.items() if key in {'HOME', 'TMPDIR', 'LANG', 'LC_ALL', 'PLAYWRIGHT_BROWSERS_PATH'}}
env['PATH'] = os.pathsep.join((str(node), str(pg), str(Path(env['HOME']) / '.cargo/bin'), '/usr/bin', '/bin', '/usr/sbin', '/sbin', '/opt/homebrew/bin'))
env.update(PGPASSWORD=passwords['POSTGRES_ADMIN_PASSWORD'], SQLX_OFFLINE='true', FRONTEND_ROOT=str(root))
source_paths = [
    'backend-fork/backend/Cargo.lock', 'backend-fork/backend/app/src/browser_payslips.rs',
    'backend-fork/backend/app/tests/auth_rest.rs', 'backend-fork/backend/app/tests/auth_rest/browser_sessions.rs',
    'backend-fork/backend/app/tests/auth_rest/browser_payslips.rs', 'backend-fork/backend/app/tests/auth_rest/genuine_browser.rs',
    'backend-fork/backend/app/tests/openapi_drift.rs', 'backend-fork/backend/crates/inbox/adapter-postgres/src/lib.rs',
    'backend-fork/backend/crates/inbox/rest/src/openapi.rs', 'backend-fork/backend/openapi/openapi.yaml',
    'tools/browser-payslips.mjs', 'tools/browser-business-restore.mjs', 'tools/browser-business-boundary.mjs', 'tools/browser-business-temporal.mjs', 'tools/test-browser-business-session.mjs',
    'tools/production-runtime.mjs', 'src/lib/server/payslips.ts', 'src/lib/server/browser-session.ts',
    'src/app/me/[context]/payslips/page.public.tsx', 'src/app/me/[context]/payslips/[id]/page.public.tsx',
    'package-lock.json', 'next-env.d.ts', 'tools/browser-business-failure.mjs', '.next/build-inputs.json', '.next/BUILD_ID', '.next/server/app-paths-manifest.json',
]
def source_snapshot():
    return {name: hashlib.sha256((root / name).read_bytes()).hexdigest() for name in source_paths}
source_before = source_snapshot()
worker_binary = Path('/private/tmp/own-issued-payslips-target-20261007/debug/console-app').resolve(strict=True)
worker_sha_before = hashlib.sha256(worker_binary.read_bytes()).hexdigest()
(evidence / 'runner.py').write_bytes(Path(__file__).read_bytes())
report = {'scope': 'single-site real native/password-auth runtime proof; browser outcomes separately counted when executed; not payroll/legal/two-site durability acceptance', 'runner_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(), 'status': 'unreached', 'commands': [], 'suites': [], 'browser_scenarios': None, 'source_inputs': source_before, 'worker_binary': {'path': str(worker_binary), 'sha256': worker_sha_before}}
owned_temporary_directory = None
owned_postmaster_pid = None
secret_strings = list(passwords.values())

def scrub(raw):
    for secret in secret_strings:
        raw = raw.replace(secret.encode(), b'[REDACTED]')
    raw = re.sub(rb'\b(?:bs1|bi1)\.[A-Za-z0-9_-]{43}\b', b'[REDACTED_SCOPED_CREDENTIAL]', raw)
    raw = re.sub(rb'\beyJ[A-Za-z0-9_-]*\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b', b'[REDACTED_BEARER]', raw)
    raw = re.sub(rb'-----BEGIN [^-]*(?:PRIVATE KEY|CERTIFICATE)-----.*?-----END [^-]+-----', b'[REDACTED_KEY_MATERIAL]', raw, flags=re.S)
    return raw

def run(name, command, extra=None, timeout=120, allow_failure=False):
    start = time.monotonic()
    logfile = evidence / (name + '.log')
    process = subprocess.Popen(command, cwd=root / 'backend-fork/backend', env={**env, **(extra or {})}, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    def drain():
        with logfile.open('wb') as stream:
            key_material = False
            for line in iter(process.stdout.readline, b''):
                if re.search(rb'-----BEGIN [^-]*(?:PRIVATE KEY|CERTIFICATE)-----', line):
                    key_material = True
                    stream.write(b'[REDACTED_KEY_MATERIAL]\n'); stream.flush()
                if key_material:
                    if b'-----END ' in line: key_material = False
                    continue
                stream.write(scrub(line)); stream.flush()
                if line.startswith(b'test ') and (b' ... ok' in line or b' ... FAILED' in line):
                    print(json.dumps({'suite': name, 'test_result': scrub(line).decode(errors='replace').strip()}), flush=True)
    reader = threading.Thread(target=drain)
    reader.start()
    try:
        code = process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        process.kill(); process.wait(); code = 124
    reader.join(timeout=10)
    if reader.is_alive():
        raise RuntimeError(name + ' output drain not confirmed')
    output = logfile.read_bytes()
    if code == 124:
        output += b'\nRUNNER_TIMEOUT\n'; logfile.write_bytes(output)
    report['commands'].append({'name': name, 'command': command, 'exit_code': code, 'duration_seconds': round(time.monotonic() - start, 3), 'output_sha256': hashlib.sha256(output).hexdigest()})
    print(json.dumps({'name': name, 'exit_code': code, 'evidence': str(evidence / (name + '.log'))}), flush=True)
    if code and not allow_failure:
        raise RuntimeError(name + ' failed; sanitized log retained')
    return output.decode(errors='replace'), code

def counts(discovery, output):
    discovered = sum(int(n) for n in re.findall(r'^(\d+) tests?, \d+ benchmarks?$', discovery, re.M))
    rows = re.findall(r'^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; \d+ measured; (\d+) filtered out;', output, re.M)
    passed, failed, ignored, filtered = [sum(int(row[i]) for row in rows) for i in range(4)]
    return dict(discovered=discovered, executed=passed+failed, passed=passed, failed=failed, skipped=ignored, filtered=filtered)

try:
    with tempfile.TemporaryDirectory(prefix='own-payslip-pg-', dir='/private/tmp') as temp:
        directory = Path(temp)
        owned_temporary_directory = directory
        report['owned_temporary_directory'] = str(directory)
        socket = directory / 'socket'
        socket.mkdir(mode=0o700)
        data = directory / 'data'
        ctl = [str(pg / 'pg_ctl'), '-D', str(data)]
        with socket_api.socket() as reservation:
            reservation.bind(('127.0.0.1', 0))
            port = reservation.getsockname()[1]
        report['owned_database_endpoint'] = {'host': '127.0.0.1', 'port': port, 'host_auth': 'scram-sha-256'}
        admin, database = 'console_buck_admin', 'own_payslips_disposable'
        pwfile = directory / 'admin-password'
        pwfile.write_text(passwords['POSTGRES_ADMIN_PASSWORD'] + '\n')
        pwfile.chmod(0o600)
        run('initdb', [str(pg / 'initdb'), '-D', str(data), '--username='+admin, '--auth-local=scram-sha-256', '--auth-host=scram-sha-256', '--pwfile='+str(pwfile), '--no-locale', '--encoding=UTF8'])
        pwfile.unlink()
        started = False
        try:
            run('start', ctl + ['-l', str(evidence / 'postgres.log'), '-o', "-c listen_addresses='127.0.0.1' -c port="+str(port)+" -c unix_socket_directories="+str(socket)+" -c log_statement=none -c log_min_error_statement=panic", '-w', 'start'])
            started = True
            owned_postmaster_pid = int((data / 'postmaster.pid').read_text().splitlines()[0])
            report['observed_postmaster_pid'] = owned_postmaster_pid
            run('createdb', [str(pg / 'createdb'), '-h', '127.0.0.1', '-p', str(port), '-U', admin, database])
            topology_env = {**passwords, 'POSTGRES_HOST': '127.0.0.1', 'POSTGRES_PORT': str(port), 'POSTGRES_DB': database, 'POSTGRES_ADMIN_USER': admin, 'POSTGRES_LOCAL_SOCKET_DIR': str(socket)}
            run('topology', ['bash', str(root / 'backend-fork/ops/postgres-reconcile-topology.sh')], topology_env)
            url = 'postgresql://' + admin + ':' + passwords['POSTGRES_ADMIN_PASSWORD'] + '@127.0.0.1:' + str(port) + '/' + database + '?' + urlencode({'options[console.sqlx_test_bootstrap]': 'buck-sqlx-superuser-v1'})
            secret_strings.append(url)
            test_env = {'DATABASE_URL': url, 'CONSOLE_TEST_OWNER_PASSWORD': passwords['CONSOLE_APP_POSTGRES_PASSWORD'], 'CONSOLE_TEST_RUNTIME_PASSWORD': passwords['CONSOLE_RT_POSTGRES_PASSWORD'], 'CONSOLE_TEST_FORCE_PASSWORD': passwords['CONSOLE_PLATFORM_FORCE_COMMAND_POSTGRES_PASSWORD']}
            for name, binary in suites:
                command = [str(binary)] + ([filters[name]] if name in filters else [])
                discovery, discovery_code = run(name+'-discovery', command + ['--list'], test_env, allow_failure=True)
                output, code = run(name, command + ['--test-threads=1', '--show-output'], test_env, timeout=3600, allow_failure=True)
                population = counts(discovery, output)
                complete = population['discovered'] > 0 and population['executed'] == population['discovered'] and population['skipped'] == 0
                report['suites'].append({'name': name, 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'status': 'passed' if discovery_code == code == 0 and complete else 'failed', **population})
                for line in output.splitlines():
                    marker = 'PRODUCT_BROWSER_SCENARIOS '
                    if line.startswith(marker):
                        report['browser_scenarios'] = json.loads(line.removeprefix(marker))
            report['status'] = 'passed' if all(suite['status'] == 'passed' for suite in report['suites']) else 'failed'
        finally:
            if started:
                _, code = run('stop', ctl + ['-m', 'immediate', '-w', 'stop'], allow_failure=True)
                report['cluster_stopped'] = code == 0
except Exception as error:
    report['status'] = 'failed'
    report['error'] = str(error)
finally:
    # PostgreSQL server diagnostics are sanitized after the owned cluster stops.
    logfile = evidence / 'postgres.log'
    if logfile.exists():
        logfile.write_bytes(scrub(logfile.read_bytes()))
    source_after = source_snapshot()
    report['source_inputs_unchanged'] = source_before == source_after
    report['worker_binary_unchanged'] = worker_sha_before == hashlib.sha256(worker_binary.read_bytes()).hexdigest()
    if not report['worker_binary_unchanged']: report['status'] = 'failed'
    if source_before != source_after:
        report['status'] = 'failed'
        report['source_inputs_changed'] = [name for name in source_before if source_before[name] != source_after[name]]
    report['temporary_credential_files_removed'] = bool(owned_temporary_directory and not owned_temporary_directory.exists() and not owned_temporary_directory.is_symlink())
    process_absent = False
    if owned_postmaster_pid is not None:
        try:
            os.kill(owned_postmaster_pid, 0)
        except ProcessLookupError:
            process_absent = True
        except PermissionError:
            pass
    report['observed_postmaster_pid_absent'] = process_absent
    if not (report.get('cluster_stopped') is True and report['temporary_credential_files_removed'] and process_absent):
        report['status'] = 'failed'
    report['credential_cleanup_scope'] = 'Observed temporary-file cleanup only; no process-memory or cryptographic-erasure claim.'
    (evidence / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'status': report['status'], 'suites': report['suites'], 'report': str(evidence / 'report.json')}), flush=True)
sys.exit(report['status'] != 'passed')
