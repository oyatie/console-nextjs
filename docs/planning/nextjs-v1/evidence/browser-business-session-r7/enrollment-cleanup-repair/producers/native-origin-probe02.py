from pathlib import Path
import datetime, hashlib, json, os, re, subprocess, time
root=Path('/Users/jasonlee/Developer/frontend/.worktrees/browser-business-session-r7')
evidence=Path(__file__).parent
name='native-origin-probe02'
case='genuine_browser::genuine_browser_discoverable_enrollment_requires_two_native_logins'
paths=['backend-fork/backend/app/tests/auth_rest/genuine_browser.rs','backend-fork/backend/Cargo.lock','backend-fork/tools/lanes/pgtest.sh','tools/production-runtime.mjs','tools/test-native-browser-enrollment.mjs','tools/enrollment-fault-deadlines.mjs','tools/test-native-browser-enrollment.test.mjs','.next/build-inputs.json']
def hashes(): return {p:hashlib.sha256((root/p).read_bytes()).hexdigest() for p in paths}
before=hashes()
env={k:v for k,v in os.environ.items() if k in {'PATH','HOME','TMPDIR','LANG','LC_ALL','RUSTUP_HOME','CARGO_HOME','PLAYWRIGHT_BROWSERS_PATH'}}
env['PATH']='/Users/jasonlee/Developer/frontend/.artifacts/acceptance/browser-business-session/r7-native-implementation/same-probe-green/docker-adapter:'+env['PATH']
env.update(DOCKER_CONTEXT='colima-nextjs-v1-auth',SQLX_OFFLINE='true',FRONTEND_ROOT=str(root),CARGO_TARGET_DIR='/Users/jasonlee/Developer/frontend/.artifacts/cargo-target',CARGO_BUILD_JOBS='2',CARGO_INCREMENTAL='0',CARGO_PROFILE_DEV_DEBUG='0',CARGO_PROFILE_TEST_DEBUG='0')
command=['bash',str(root/'backend-fork/tools/lanes/pgtest.sh'),str(root/'backend-fork'),'/Users/jasonlee/.cargo/bin/cargo','+1.98.1','test','--locked','--offline','--features','frontend-e2e','-p','console-app','--test','auth_rest',case,'--','--exact','--test-threads=1','--nocapture']
started=time.monotonic()
with (evidence/(name+'.log')).open('xb') as log: result=subprocess.run(command,cwd=root,env=env,stdout=log,stderr=subprocess.STDOUT)
after=hashes(); text=(evidence/(name+'.log')).read_text()
rows=re.findall(r'^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; \d+ measured; (\d+) filtered out;',text,re.M)
counts={k:sum(int(row[i]) for row in rows) for i,k in enumerate(['passed','failed','ignored','filtered'])}
report={'schema':'actual-native-origin-consumer-probe/1','at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'command':command,'cwd':str(root),'case':case,'exit_code':result.returncode,'duration_seconds':time.monotonic()-started,'counts':counts,'inputs_before':before,'inputs_after':after,'inputs_unchanged':before==after,'raw_log_sha256':hashlib.sha256((evidence/(name+'.log')).read_bytes()).hexdigest(),'cleanup_reported':bool(re.search(r'^clean: console-conformance-\d+ removed with its volume$',text,re.M)),'scope':'One exact genuine native enrollment consumer using production staging, disposable PostgreSQL and real virtual WebAuthn. Selection intends one case only; actual executed/filtered counts come solely from observed Rust summaries. Not full121/17 or release acceptance.'}
with (evidence/(name+'.json')).open('x') as f:json.dump(report,f,indent=2);f.write('\n')
print(json.dumps(report,indent=2));raise SystemExit(result.returncode)
