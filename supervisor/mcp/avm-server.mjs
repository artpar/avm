import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { spawn } from 'node:child_process';
import readline from 'node:readline';

const config=JSON.parse(await readFile(process.argv[2],'utf8')); validateConfig(config);
const tools=[
  tool('avm_capture','Capture the authoritative VM framebuffer and return it as an image.',{}),
  tool('avm_click','Move the real guest pointer and click.',{x:{type:'integer',minimum:0},y:{type:'integer',minimum:0}},['x','y']),
  tool('avm_type','Type text through the real guest keyboard.',{text:{type:'string',maxLength:4096}},['text']),
  tool('avm_key','Send one Linux input keycode through QEMU.',{keycode:{type:'integer',minimum:0,maximum:767},mode:{type:'string',enum:['press','down','up']}},['keycode']),
  tool('avm_history','Read recorded cross-source history.',{lastDurationMs:{type:'integer',minimum:1},source:{type:'array',items:{type:'string'}}}),
  tool('avm_query','Run a structured experience query.',{query:{type:'object'}},['query']),
  tool('avm_accessibility','Observe a fresh native accessibility tree and events.',{durationMs:{type:'integer',minimum:1,maximum:60000}}),
  tool('avm_browser_observe','Record a CDP browser snapshot, network, console, performance, screenshot, and trace.',{durationMs:{type:'integer',minimum:1,maximum:60000}})
];

const lines=readline.createInterface({input:process.stdin,crlfDelay:Infinity});
for await(const line of lines){if(!line.trim())continue;let request;try{request=JSON.parse(line)}catch{continue}if(request.id===undefined)continue;try{let result;if(request.method==='initialize')result={protocolVersion:request.params?.protocolVersion||'2025-06-18',capabilities:{tools:{listChanged:false}},serverInfo:{name:'avm-workstation',version:'1'}};else if(request.method==='tools/list')result={tools};else if(request.method==='tools/call')result=await callTool(request.params?.name,request.params?.arguments||{});else throw new Error(`unsupported method ${request.method}`);send({jsonrpc:'2.0',id:request.id,result})}catch(error){send({jsonrpc:'2.0',id:request.id,result:{content:[{type:'text',text:error.message}],isError:true}})}}

function tool(name,description,properties,required=[]){return{name,description,inputSchema:{type:'object',properties,required,additionalProperties:false}}}
function send(value){process.stdout.write(`${JSON.stringify(value)}\n`)}
function validateConfig(value){for(const key of ['project','zone','instance','remoteAvm','remoteRun'])if(typeof value[key]!=='string'||!value[key])throw new Error(`config missing ${key}`);for(const key of ['project','zone','instance'])if(!/^[a-zA-Z0-9._-]+$/.test(value[key]))throw new Error(`unsafe ${key}`);for(const key of ['remoteAvm','remoteRun'])if(!value[key].startsWith('/')||/[\n\r\0]/.test(value[key]))throw new Error(`unsafe ${key}`)}
function shellQuote(value){return `'${String(value).replaceAll("'","'\\''")}'`}
async function callTool(name,args){
  if(name==='avm_capture')return capture();
  if(name==='avm_click')return text(await runAvm(['act-click','--run',config.remoteRun,'--x',integer(args.x),'--y',integer(args.y)]));
  if(name==='avm_type')return text(await runAvm(['act-type','--run',config.remoteRun,'--text',String(args.text)]));
  if(name==='avm_key')return text(await runAvm(['act-key','--run',config.remoteRun,'--keycode',integer(args.keycode),'--mode',args.mode||'press']));
  if(name==='avm_history'){const command=['history','--run',config.remoteRun,'--last-duration-ms',integer(args.lastDurationMs||10000)];for(const source of args.source||[])command.push('--source',String(source));return text(await runAvm(command))}
  if(name==='avm_query')return text(await query(args.query));
  if(name==='avm_accessibility')return text(await runAvm(['accessibility-observe','--run',config.remoteRun,'--duration-ms',integer(args.durationMs||5000)]));
  if(name==='avm_browser_observe')return text(await runAvm(['browser-observe','--run',config.remoteRun,'--endpoint',config.browserEndpoint||'http://127.0.0.1:9222','--script',config.remoteBrowserScript||'/home/artpar/avm/supervisor/browser/observer.mjs','--duration-ms',integer(args.durationMs||5000)]));
  throw new Error(`unknown AVM tool ${name}`)
}
function integer(value){if(!Number.isSafeInteger(Number(value)))throw new Error('expected integer');return String(value)}
function text(output){return{content:[{type:'text',text:output}]}}
async function runAvm(args){const command=`cd ${shellQuote(dirname(config.remoteAvm))} && ${[config.remoteAvm,...args].map(shellQuote).join(' ')}`;return run('gcloud',['compute','ssh',config.instance,'--project',config.project,'--zone',config.zone,'--command',command])}
async function capture(){const remote=`/tmp/avm-mcp-${process.pid}-${Date.now()}.png`;await runAvm(['capture','--run',config.remoteRun,'--output',remote]);const temporary=await mkdtemp(join(tmpdir(),'avm-mcp-'));const local=join(temporary,'frame.png');try{await run('gcloud',['compute','scp',`${config.instance}:${remote}`,local,'--project',config.project,'--zone',config.zone]);const data=await readFile(local);return{content:[{type:'image',data:data.toString('base64'),mimeType:'image/png'},{type:'text',text:'Authoritative QEMU framebuffer capture.'}]} }finally{await rm(temporary,{recursive:true,force:true})}}
async function query(value){const temporary=await mkdtemp(join(tmpdir(),'avm-query-'));const local=join(temporary,'query.json');const remote=`/tmp/avm-query-${process.pid}-${Date.now()}.json`;try{await writeFile(local,JSON.stringify(value));await run('gcloud',['compute','scp',local,`${config.instance}:${remote}`,'--project',config.project,'--zone',config.zone]);return runAvm(['experience-query','--run',config.remoteRun,'--input',remote])}finally{await rm(temporary,{recursive:true,force:true})}}
function run(program,args){return new Promise((resolveRun,reject)=>{const child=spawn(program,args,{stdio:['ignore','pipe','pipe']});let stdout='',stderr='';child.stdout.on('data',chunk=>stdout+=chunk);child.stderr.on('data',chunk=>stderr+=chunk);child.on('error',reject);child.on('exit',code=>code===0?resolveRun(stdout.trim()):reject(new Error(`${program} exited ${code}: ${stderr.trim()}`)))})}
