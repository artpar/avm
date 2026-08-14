import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import assert from 'node:assert/strict';

const directory = await mkdtemp(join(tmpdir(), 'avm-board-'));
const port = 31000 + Math.floor(Math.random() * 1000);
const server = spawn(process.execPath, ['server.mjs'], { cwd:new URL('.',import.meta.url), env:{...process.env,PORT:String(port),BOARD_STATE:join(directory,'state.json'),BOARD_LATENCY_MS:'1'}, stdio:['ignore','pipe','inherit'] });
try {
  await new Promise((resolve,reject) => { server.stdout.on('data',resolve); server.once('exit',code=>reject(new Error(`server exited ${code}`))); });
  const api = async (path, options={}) => { const response=await fetch(`http://127.0.0.1:${port}${path}`,{headers:{'content-type':'application/json'},...options}); const body=await response.json(); assert.equal(response.ok,true,JSON.stringify(body)); return body; };
  const before=await api('/api/state');
  const created=await api('/api/cards',{method:'POST',body:JSON.stringify({projectId:'atlas',title:'Persistence check',priority:'high'})});
  await api(`/api/cards/${created.card.id}`,{method:'PATCH',body:JSON.stringify({columnId:'done',description:'Moved through API'})});
  const changed=await api('/api/state'); assert.equal(changed.cards.find(card=>card.id===created.card.id).columnId,'done');
  const undone=await api('/api/undo',{method:'POST',body:'{}'}); assert.equal(undone.cards.find(card=>card.id===created.card.id).columnId,'backlog'); assert.ok(undone.revision>before.revision);
  const page=await fetch(`http://127.0.0.1:${port}/`).then(response=>response.text()); for (const marker of ['Issue board','dialog','Search','Undo']) assert.ok(page.includes(marker));
  console.log(JSON.stringify({ok:true,revision:undone.revision,cardId:created.card.id}));
} finally { server.kill('SIGTERM'); await rm(directory,{recursive:true,force:true}); }
