// External diagnosis only. Current runner, unchanged parent deadlines and input.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
const root='/Users/jasonlee/Developer/frontend/.worktrees/browser-business-session-r7';
const output=new URL('./phase-probe01.json',import.meta.url);
const owned=await mkdtemp(path.join(tmpdir(),'enrollment-phase-probe-'));
const started=performance.now();
const phases=[];
const stamp=(phase,extra={})=>phases.push({phase,elapsed_ms:Math.round(performance.now()-started),...extra});
const env=Object.fromEntries(['PATH','HOME','TMPDIR','LANG','LC_ALL','PLAYWRIGHT_BROWSERS_PATH'].filter(k=>process.env[k]!==undefined).map(k=>[k,process.env[k]]));
const child=spawn(process.execPath,[path.join(root,'tools/test-native-browser-enrollment.mjs')],{cwd:root,env:{...env,TMPDIR:owned},stdio:['pipe','pipe','pipe']});
stamp('spawn');
let stderrBytes=0;
child.stderr.on('data',bytes=>{stderrBytes+=bytes.length;});
const exited=once(child,'exit');
const input=child.stdout[Symbol.asyncIterator]();
let buffered=Buffer.alloc(0),sequence=0,fallbackFired=false;
async function read(){
 while(true){
  if(buffered.length>=4){const length=buffered.readUInt32BE();assert.ok(length>0&&length<=65536);
   if(buffered.length>=length+4){const frame=JSON.parse(buffered.subarray(4,length+4).toString('utf8'));buffered=buffered.subarray(length+4);assert.equal(frame.v,1);assert.equal(frame.seq,sequence++);assert.ok(sequence<=128);return frame;}}
  const next=await input.next();assert.ok(!next.done,'runner ended before acknowledgement');assert.ok(buffered.length+next.value.length<=65540);buffered=Buffer.concat([buffered,next.value]);
 }
}
const deadline=setTimeout(()=>{fallbackFired=true;stamp('parent_fallback_sigterm');child.kill('SIGTERM');},25000);
const force=setTimeout(()=>{fallbackFired=true;stamp('parent_fallback_sigkill');child.kill('SIGKILL');},35000);
let error=null;
try{
 assert.equal((await read()).kind,'origin');stamp('origin');
 assert.equal((await read()).kind,'ready');stamp('ready');
 const body=Buffer.from(JSON.stringify({v:2,seq:0,kind:'register',actor:'a',mode:'legacy',ceremony:'11111111-1111-4111-8111-111111111111',options:{publicKey:{rp:{id:'localhost'}}}}));
 const header=Buffer.alloc(4);header.writeUInt32BE(body.length);stamp('fault_injected',{fault:'wrong-version'});child.stdin.write(Buffer.concat([header,body]));
 const cleaned=await read();assert.equal(cleaned.kind,'cleaned');stamp('cleaned',{outcome:cleaned.outcome,creation_attempts:cleaned.creation_attempts});
 assert.equal(cleaned.creation_attempts,0);assert.equal(cleaned.outcome,'failed');child.stdin.end();
 const [code,signal]=await exited;stamp('exit',{code,signal});assert.equal(code,1);assert.equal(signal,null);assert.equal(fallbackFired,false,'fallback intervention cannot prove requested cleanup');
 assert.deepEqual(await readdir(owned),[]);stamp('owned_temp_empty');
}catch(e){error={name:e.name,message:e.message};}
finally{
 clearTimeout(deadline);clearTimeout(force);child.stdin.destroy();
 if(child.exitCode===null&&child.signalCode===null){child.kill('SIGTERM');const forced=setTimeout(()=>child.kill('SIGKILL'),6000);await exited;clearTimeout(forced);}
 const remaining=(await readdir(owned)).length;stamp('final_parent_cleanup',{remaining_entries:remaining});await rm(owned,{recursive:true,force:true});
}
const report={schema:'enrollment-parent-phase-diagnostic/1',node:process.version,head:'f5c29cc9ccb1d463f5ba76ebbf746b024311bd37',scope:'External single real-runner diagnosis; no native enrollment acceptance or historical-cause claim',unchanged_parent_ms:{fallback:25000,force:35000},phases,stderr_bytes:stderrBytes,fallback_fired:fallbackFired,error};
await writeFile(output,JSON.stringify(report,null,2)+'\n',{flag:'wx'});console.log(JSON.stringify(report,null,2));process.exitCode=error?1:0;
