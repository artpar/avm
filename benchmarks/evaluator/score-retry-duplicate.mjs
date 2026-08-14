import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawn } from 'node:child_process';

const workspace=resolve(process.argv[2]||''); if(!process.argv[2]) throw new Error('workspace required');
const temp=await mkdtemp(join(tmpdir(),'avm-score-')); const targetPort=34000+Math.floor(Math.random()*500); const proxyPort=targetPort+500;
const run=(command,args,cwd,env={})=>new Promise(resolveRun=>{const child=spawn(command,args,{cwd,env:{...process.env,...env},stdio:['ignore','pipe','pipe']});let stdout='',stderr='';child.stdout.on('data',chunk=>stdout+=chunk);child.stderr.on('data',chunk=>stderr+=chunk);child.on('exit',exitCode=>resolveRun({exitCode,stdout,stderr}))});
let functionalDefects=0,regressions=0,duplicateCount=null;
const check=await run('npm',['run','check'],workspace); if(check.exitCode!==0) regressions++;
const target=spawn(process.execPath,['server.mjs'],{cwd:workspace,env:{...process.env,PORT:String(targetPort),BOARD_STATE:join(temp,'state.json'),BOARD_LATENCY_MS:'1'},stdio:['ignore','pipe','pipe']});
try{
  await new Promise((resolveReady,reject)=>{target.stdout.once('data',resolveReady);target.once('exit',code=>reject(new Error(`target exited ${code}`)))});
  const profile=resolve(new URL('profiles/double-action.json',import.meta.url).pathname);
  const proxy=spawn(process.execPath,[resolve(new URL('fault-proxy.mjs',import.meta.url).pathname)],{env:{...process.env,EVALUATOR_PORT:String(proxyPort),TARGET_ORIGIN:`http://127.0.0.1:${targetPort}`,FAULT_PROFILE:profile},stdio:['ignore','pipe','pipe']});
  try{
    await new Promise((resolveReady,reject)=>{proxy.stdout.once('data',resolveReady);proxy.once('exit',code=>reject(new Error(`proxy exited ${code}`)))});
    const before=await fetch(`http://127.0.0.1:${proxyPort}/api/state`).then(r=>r.json());
    const response=await fetch(`http://127.0.0.1:${proxyPort}/api/cards`,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({projectId:'atlas',title:'Retry-safe card',description:'one logical action',priority:'high'})});
    if(!response.ok) functionalDefects++; const after=await fetch(`http://127.0.0.1:${proxyPort}/api/state`).then(r=>r.json()); duplicateCount=after.cards.length-before.cards.length; if(duplicateCount!==1) functionalDefects++;
  }finally{proxy.kill('SIGTERM')}
}catch{functionalDefects++}finally{target.kill('SIGTERM');await rm(temp,{recursive:true,force:true})}
console.log(JSON.stringify({functionalDefects,hiddenDefectsDiscovered:duplicateCount===1?1:0,userFacingDefects:functionalDefects,temporalDefects:0,incorrectRequirementInterpretations:0,regressions,rework:null,failedAttempts:null,timeMs:null,toolCalls:null,modelTokens:null,productInteractions:null,diagnosisAccuracy:duplicateCount===1?1:0,humanInterventions:0,catastrophicImplementations:0,detail:{duplicateCount,checkExitCode:check.exitCode}}));
