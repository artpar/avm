import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import assert from 'node:assert/strict';

const temp=await mkdtemp(join(tmpdir(),'avm-evaluator-')); const targetPort=33000+Math.floor(Math.random()*500); const proxyPort=targetPort+500;
const profile=join(temp,'profile.json'); await writeFile(profile,JSON.stringify({doubleAction:true}));
const target=spawn(process.execPath,['server.mjs'],{cwd:new URL('../target-app',import.meta.url),env:{...process.env,PORT:String(targetPort),BOARD_STATE:join(temp,'state.json'),BOARD_LATENCY_MS:'1'},stdio:['ignore','pipe','inherit']});
await new Promise((resolve,reject)=>{target.stdout.once('data',resolve);target.once('exit',code=>reject(new Error(`target exited ${code}`)))});
const proxy=spawn(process.execPath,['fault-proxy.mjs'],{cwd:new URL('.',import.meta.url),env:{...process.env,EVALUATOR_PORT:String(proxyPort),TARGET_ORIGIN:`http://127.0.0.1:${targetPort}`,FAULT_PROFILE:profile},stdio:['ignore','pipe','inherit']});
try {
  await new Promise((resolve,reject)=>{proxy.stdout.once('data',resolve);proxy.once('exit',code=>reject(new Error(`proxy exited ${code}`)))});
  const before=await fetch(`http://127.0.0.1:${proxyPort}/api/state`).then(r=>r.json());
  const response=await fetch(`http://127.0.0.1:${proxyPort}/api/cards`,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({projectId:'atlas',title:'one user action'})}); assert.equal(response.ok,true);
  const after=await fetch(`http://127.0.0.1:${proxyPort}/api/state`).then(r=>r.json());
  assert.equal(after.cards.length-before.cards.length,2);
  const source=await fetch(`http://127.0.0.1:${proxyPort}/app.js`).then(r=>r.text()); assert.ok(source.includes('function render'));
  console.log(JSON.stringify({ok:true,oneRequestCreated:2,evaluatorOutsideCandidate:true}));
} finally { proxy.kill('SIGTERM'); target.kill('SIGTERM'); await rm(temp,{recursive:true,force:true}); }
