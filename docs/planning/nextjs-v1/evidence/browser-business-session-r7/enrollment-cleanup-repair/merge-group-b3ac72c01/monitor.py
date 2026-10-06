from pathlib import Path
import datetime, hashlib, json, subprocess, time
ROOT = Path(__file__).parent
REPO = 'oyatie/console-nextjs'
HEAD = 'b3ac72c41d3d5dca9ff9c2334543098c2c8bd177'
ACCEPTED = 'f5c29cc9ccb1d463f5ba76ebbf746b024311bd37'
TREE = 'f8845f3b37cbc923c9354cc3a46ad9be13035c88'
RUN = 37403914643
JOB = 112077080738
QUERY = 'query{repository(owner:"oyatie",name:"console-nextjs"){pullRequest(number:16){number headRefOid state merged mergedAt mergeCommit{oid} mergeQueueEntry{id state position enqueuedAt jump headCommit{oid}} autoMergeRequest{enabledAt}}}}'

def gh(*args):
    return subprocess.run(['gh', *args], capture_output=True, timeout=55)

def preserve(name, data):
    with (ROOT / name).open('xb') as f:
        f.write(data)

def capture(name, *args):
    for attempt in range(1, 7):
        result = gh(*args)
        if result.returncode == 0:
            preserve(name, result.stdout)
            return result.stdout
        preserve(name+f'.attempt{attempt:02}.stderr', result.stderr)
        preserve(name+f'.attempt{attempt:02}.stdout', result.stdout)
        time.sleep(20)
    raise RuntimeError('Read-only evidence acquisition unavailable: '+name)

last = None
while True:
    proc = gh('run', 'view', str(RUN), '--repo', REPO, '--json', 'databaseId,headSha,event,status,conclusion,createdAt,updatedAt,url,jobs')
    if proc.returncode:
        print(json.dumps({'poll_error':proc.returncode,'stderr':proc.stderr.decode(errors='replace')[:500]}),flush=True)
        time.sleep(45)
        continue
    meta = json.loads(proc.stdout)
    assert meta['headSha'] == HEAD and meta['event'] == 'merge_group' and meta['databaseId'] == RUN
    job = next(j for j in meta['jobs'] if j['databaseId'] == JOB)
    assert job['name'] == 'source-and-inquiry'
    current = [s['name'] for s in job['steps'] if s['status'] == 'in_progress']
    failed = [s['name'] for s in job['steps'] if s['conclusion'] not in ('','success','skipped')]
    state = (meta['status'],job['status'],tuple(current),tuple(failed),meta['conclusion'])
    if state != last:
        record = {'observed_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'head':HEAD,'run_id':RUN,'job_id':JOB,'run_status':meta['status'],'job_status':job['status'],'conclusion':meta['conclusion'],'reported_steps':len(job['steps']),'completed_steps':sum(s['status']=='completed' for s in job['steps']),'current':current,'failed':failed,'scope':'Progress only; missing/partial rows are unknown, not skipped or rollback.'}
        with (ROOT/'progress-observations.jsonl').open('a') as f:f.write(json.dumps(record)+'\n')
        print(json.dumps(record),flush=True)
        last = state
    if meta['status']=='completed' and job['status']=='completed':
        preserve(f'run-{RUN}.json',proc.stdout)
        direct = json.loads(capture(f'job-{JOB}.json','api',f'repos/{REPO}/actions/jobs/{JOB}'))
        assert direct['id']==JOB and direct['run_id']==RUN and direct['head_sha']==HEAD and direct['status']=='completed'
        capture(f'job-{JOB}-direct01.log','api','--allow-escape-sequences',f'repos/{REPO}/actions/jobs/{JOB}/logs')
        capture(f'run-{RUN}-full01.zip','api',f'repos/{REPO}/actions/runs/{RUN}/logs')
        print(json.dumps({'terminal_evidence_captured':True,'run_id':RUN,'job_id':JOB,'conclusion':meta['conclusion'],'directory':str(ROOT),'count_audit':'pending; no acceptance inferred'}),flush=True)
        attempts = 20 if meta['conclusion']=='success' else 1
        for attempt in range(1,attempts+1):
            pr_raw = capture(f'pr-terminal-readback{attempt:02}.json','api','graphql','-f','query='+QUERY)
            pr = json.loads(pr_raw)['data']['repository']['pullRequest']
            assert pr['number']==16 and pr['headRefOid']==ACCEPTED
            if pr['merged']:
                assert pr['state']=='MERGED' and pr['mergeCommit'] is not None
                commit_sha = pr['mergeCommit']['oid']
                commit = json.loads(capture('merged-commit.json','api',f'repos/{REPO}/git/commits/{commit_sha}'))
                assert commit['sha']==commit_sha and commit['tree']['sha']==TREE
                main = json.loads(capture('main-ref-at-merge.json','api',f'repos/{REPO}/git/ref/heads/main'))
                print(json.dumps({'merge_readback':True,'merged_commit':commit_sha,'merged_tree':commit['tree']['sha'],'main_ref':main['object']['sha'],'scope':'Exact merged tree binding only; CI count audit remains separate.'}),flush=True)
                raise SystemExit(0)
            if meta['conclusion']!='success':break
            time.sleep(45)
        print(json.dumps({'merge_readback':False,'scope':'Terminal CI captured; no merge inferred. Follow-up readback required.'}),flush=True)
        raise SystemExit(0)
    time.sleep(45)
