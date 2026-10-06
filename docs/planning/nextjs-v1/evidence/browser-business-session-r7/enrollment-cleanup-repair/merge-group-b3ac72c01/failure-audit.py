from pathlib import Path
import collections,datetime,hashlib,json,re,subprocess,zipfile
ROOT=Path(__file__).parent
RUN=37403914643
JOB=112077080738
HEAD='b3ac72c41d3d5dca9ff9c2334543098c2c8bd177'
ACCEPTED='f5c29cc9ccb1d463f5ba76ebbf746b024311bd37'
def read(name):return json.loads((ROOT/name).read_text())
def fp(path):
 b=path.read_bytes();return {'path':str(path),'bytes':len(b),'sha256':hashlib.sha256(b).hexdigest()}
run=read(f'run-{RUN}.json');job=read(f'job-{JOB}.json');check=read('check-terminal01.json');pr=read('pr-terminal-readback01.json')['data']['repository']['pullRequest'];binding=read('queue-tree-binding01.json')
assert run['databaseId']==RUN and run['headSha']==HEAD and run['event']=='merge_group' and run['status']=='completed' and run['conclusion']=='failure'
assert job['id']==JOB and job['run_id']==RUN and job['head_sha']==HEAD and job['status']=='completed' and job['conclusion']=='failure'
assert check['id']==JOB and check['app']['id']==15368 and check['head_sha']==HEAD and check['name']=='source-and-inquiry' and check['conclusion']=='failure'
assert len(job['steps'])==42 and all(s['status']=='completed' for s in job['steps'])
assert collections.Counter(s['conclusion'] for s in job['steps'])=={'success':16,'failure':1,'skipped':25}
assert [s['number'] for s in job['steps'] if s['conclusion']=='failure']==[14]
assert pr['headRefOid']==ACCEPTED and pr['state']=='OPEN' and pr['merged'] is False and pr['mergedAt'] is None and pr['mergeCommit'] is None and pr['mergeQueueEntry'] is None
raw_path=ROOT/f'job-{JOB}-direct01.log';raw=raw_path.read_bytes()
with zipfile.ZipFile(ROOT/f'run-{RUN}-full01.zip') as archive:
 assert archive.read('0_source-and-inquiry.txt')==raw
rows=[]
for number,line in enumerate(raw.decode().splitlines(),1):
 text=re.sub(r'^\ufeff?\d{4}-\d\d-\d\dT\S+\s?', '', line)
 text=re.sub(r'\x1b\[[0-9;]*m','',text)
 rows.append({'line':number,'text':text})
marker='##[group]Run node --test tools/test-native-browser-enrollment.test.mjs'
starts=[i for i,r in enumerate(rows) if r['text'].startswith(marker)];assert len(starts)==1
start=starts[0];end=next((i for i in range(start+1,len(rows)) if rows[i]['text'].startswith('##[group]Run ')),len(rows));group=rows[start:end]
counts={m.group(1):int(m.group(2)) for r in group for m in [re.match(r'^ℹ (tests|suites|pass|fail|cancelled|skipped|todo) (\d+)$',r['text'])] if m}
assert counts=={'tests':10,'suites':0,'pass':1,'fail':9,'cancelled':0,'skipped':0,'todo':0}
summary_start=next(i for i,r in enumerate(group) if r['text'].startswith('ℹ tests '))
cases=[{'name':m.group(2),'outcome':'passed' if m.group(1)=='✔' else 'failed','duration_ms':m.group(3),'raw_line':r['line']} for r in group[:summary_start] for m in [re.match(r'^(✔|✖) (.+) \(([0-9.]+)ms\)$',r['text'])] if m]
assert len(cases)==10 and sum(c['outcome']=='passed' for c in cases)==1 and sum(c['outcome']=='failed' for c in cases)==9
assert len({c['name'] for c in cases})==10
fallback_rows=[r['line'] for r in group if 'AssertionError [ERR_ASSERTION]: fallback intervention cannot prove requested cleanup' in r['text']]
assert len(fallback_rows)==8
ready_rows=[r['line'] for r in group if "'cleaned' !== 'ready'" in r['text']];assert len(ready_rows)==1
assert not any('test result:' in r['text'] for r in rows)
assert binding['synthetic_head']==HEAD and binding['accepted_head']==ACCEPTED and binding['exact_tree']=='f8845f3b37cbc923c9354cc3a46ad9be13035c88'
assert subprocess.check_output(['git','rev-parse','HEAD']).decode().strip()==ACCEPTED and subprocess.check_output(['git','status','--porcelain']).decode()==''
record={'schema':'r7-merge-group-terminal-failure/1','created_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'verdict':'REJECT_MERGE_GROUP_ACCEPTANCE; REQUIRED_ENROLLMENT_PROCESS_CLEANUP_GATE_FAILED; NOT_MERGED','repository':'oyatie/console-nextjs','pull_request':16,'accepted_head':ACCEPTED,'synthetic_head':HEAD,'exact_tree':binding['exact_tree'],'run_id':RUN,'job_id':JOB,'run_url':run['url'],'event':'merge_group','status':'completed','conclusion':'failure','required_check_app_id':15368,'step_accounting':{'reported':42,'success':16,'failure':1,'skipped':25,'pending':0},'failed_steps':[s for s in job['steps'] if s['conclusion']=='failure'],'skipped_steps':[{'number':s['number'],'name':s['name']} for s in job['steps'] if s['conclusion']=='skipped'],'enrollment_process_fault_units':{'counts':counts,'cases':cases,'native_api_or_enrollment_acceptance':False,'observed_assertions':{'fallback_intervention_eight_cases':{'raw_lines':fallback_rows,'source':'tools/test-native-browser-enrollment.test.mjs:94','actual':True,'expected':False},'wrong_kind_not_ready':{'raw_lines':ready_rows,'source':'tools/test-native-browser-enrollment.test.mjs:62','actual':'cleaned','expected':'ready'}}},'native_authentication_producer':{'executed':0,'discovered':None,'reason':'Step19 was skipped after step14 failure; no native Rust test summaries emitted.'},'canonical_browser':{'registered_scenarios_in_source':17,'executed':0,'discovered':None,'report_emitted':False,'reason':'Containing native producer was skipped; prior PR-head121/17 proof is not merge-group proof.'},'queue_readback':{'state':pr['state'],'merged':False,'merge_commit':None,'queue_entry':None,'auto_merge_request':None,'meaning':'Previously verified queue entry was absent after failed merge-group. PR remains open; no manual dequeue performed by this auditor.'},'observed_failure_not_root_cause':'Eight units needed parent fallback; wrong-kind did not reach ready. This does not establish whether staging/startup, cleanup, resource contention or another cause triggered it.','read_only_hypothesis':{'status':'UNPROVEN','source':['tools/test-native-browser-enrollment.test.mjs:55','tools/test-native-browser-enrollment.mjs:145','tools/test-native-browser-enrollment.mjs:169'],'description':'25s fallback begins afterspawn before staging/origin/ready. Phase timestamps are needed to distinguish startup consumption from cleanup failure; no timers/inputs/outcomes changed and no probes run by auditor.'},'raw_log':fp(raw_path),'raw_log_archive':fp(ROOT/f'run-{RUN}-full01.zip'),'run_metadata':fp(ROOT/f'run-{RUN}.json'),'job_metadata':fp(ROOT/f'job-{JOB}.json'),'required_check':fp(ROOT/'check-terminal01.json'),'pr_readback':fp(ROOT/'pr-terminal-readback01.json'),'verified_queue_binding':fp(ROOT/'queue-tree-binding01.json'),'auditor_script':fp(Path(__file__)),'mutations':{'source':False,'github':False,'workflow_rerun':False,'queue':False,'merge':False},'scope_limit':'Exact rejected merge-group only; successful priorPR-head CI and queue admission remain historical facts, not current delivery. Inbox REST2 coverage is still outside inherited workflow.'}
out=ROOT/'terminal-failure-audit01.json'
with out.open('x') as f:json.dump(record,f,indent=2);f.write('\n')
files=[fp(p) for p in sorted(ROOT.iterdir()) if p.is_file()]
manifest={'schema':'r7-merge-group-failure-custody/1','created_at':record['created_at'],'run_id':RUN,'job_id':JOB,'synthetic_head':HEAD,'failure_audit':fp(out),'artifacts':files,'mutations':record['mutations']}
mp=ROOT/'terminal-failure-custody01.json'
with mp.open('x') as f:json.dump(manifest,f,indent=2);f.write('\n')
print(json.dumps({'verdict':record['verdict'],'artifact':fp(out),'custody':fp(mp),'counts':counts,'step_accounting':record['step_accounting'],'not_merged':True,'queue_entry':None},indent=2))
