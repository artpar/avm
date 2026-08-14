import http from 'node:http';
import { readFile } from 'node:fs/promises';

const listenPort = Number(process.env.EVALUATOR_PORT || 3001);
const upstream = new URL(process.env.TARGET_ORIGIN || 'http://127.0.0.1:3000');
const profile = process.env.FAULT_PROFILE ? JSON.parse(await readFile(process.env.FAULT_PROFILE, 'utf8')) : {};

const sleep = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));

async function forward(request, body) {
  const headers = { ...request.headers }; delete headers.host; delete headers['content-length'];
  return fetch(new URL(request.url, upstream), { method:request.method, headers, body:body.length ? body : undefined, redirect:'manual' });
}

function injection() {
  const scripts = [];
  if (profile.temporalDragDelayMs) scripts.push(`let avmDropBypass=false;document.addEventListener('drop',event=>{if(avmDropBypass)return;event.preventDefault();event.stopImmediatePropagation();const target=event.target;setTimeout(()=>{avmDropBypass=true;target.dispatchEvent(new Event('drop',{bubbles:true,cancelable:true}));avmDropBypass=false},${Number(profile.temporalDragDelayMs)})},true);`);
  if (profile.focusLoss) scripts.push(`document.getElementById('editor').addEventListener('close',()=>setTimeout(()=>{document.body.tabIndex=-1;document.body.focus()},0));`);
  if (profile.clippedDialog) scripts.push(`const s=document.createElement('style');s.textContent='@media(max-width:900px){dialog{height:210px;overflow:hidden}dialog form{min-height:520px}}';document.head.append(s);`);
  if (profile.flickerFrames) scripts.push(`new MutationObserver(()=>{const card=document.querySelector('.card.pending h3');if(!card||card.dataset.flicker)return;card.dataset.flicker='1';const text=card.textContent;card.textContent='Wrong project';let left=${Number(profile.flickerFrames)};const tick=()=>left--<=0?card.textContent=text:requestAnimationFrame(tick);requestAnimationFrame(tick)}).observe(document.getElementById('board'),{subtree:true,childList:true});`);
  if (profile.sequenceFailure) scripts.push(`let seq=[];document.addEventListener('click',event=>{const project=event.target.closest('#projects button');if(project){seq.push(project.textContent);seq=seq.slice(-3);if(seq.join('|')==='Atlas launch|Relay reliability|Atlas launch')document.getElementById('undo').disabled=true}},true);`);
  if (profile.hiddenShortcut) scripts.push(`document.addEventListener('keydown',event=>{if(event.altKey&&event.key.toLowerCase()==='b')document.getElementById('board').style.visibility='hidden'});`);
  return scripts.join('\n');
}

const server = http.createServer(async (request, response) => {
  try {
    const chunks=[]; for await (const chunk of request) chunks.push(chunk); const body=Buffer.concat(chunks);
    if (profile.networkDelayMs && request.method==='PATCH' && request.url.startsWith('/api/cards/')) await sleep(Number(profile.networkDelayMs));
    if (profile.earlySuccess && request.method==='PATCH' && request.url.startsWith('/api/cards/')) {
      void (async()=>{ await sleep(Number(profile.persistenceDelayMs||300)); await forward(request,body); })();
      response.writeHead(200,{'content-type':'application/json'}); return response.end(JSON.stringify({card:JSON.parse(body),revision:-1,evaluatorEarlyResponse:true}));
    }
    const first=await forward(request,body);
    if (profile.doubleAction && request.method==='POST' && request.url==='/api/cards') await forward(request,body);
    let payload=Buffer.from(await first.arrayBuffer());
    if (request.method==='GET' && request.url==='/app.js') payload=Buffer.concat([payload,Buffer.from(`\n${injection()}\n`)]);
    const headers=Object.fromEntries(first.headers); delete headers['content-length'];
    response.writeHead(first.status,headers); response.end(payload);
  } catch(error) { response.writeHead(502,{'content-type':'application/json'}); response.end(JSON.stringify({error:error.message})); }
});
server.listen(listenPort,'0.0.0.0',()=>console.log(`fault proxy listening on ${listenPort}`));
