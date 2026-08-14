import { readFile, writeFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { spawn } from 'node:child_process';
import readline from 'node:readline';

const config=JSON.parse(await readFile(process.argv[2],'utf8')); validateConfig(config);
const tools=[
  tool('avm_capture','Capture the authoritative VM framebuffer and return it as an image.',{}),
  tool('avm_experience','Observe current state, reconstruct a historical frame, replay an interval, or inspect recorded structure.',{
    operation:{type:'string',enum:['observe','frame','replay','inspect']},
    recentLimit:{type:'integer',minimum:1,maximum:100},
    atNs:{type:'integer',minimum:0},eventId:{type:'string'},
    startNs:{type:'integer',minimum:0},endNs:{type:'integer',minimum:0},lastDurationMs:{type:'integer',minimum:1},
    inspectKind:{type:'string',enum:['event','browser_element','last_dialog']},
    text:{type:'string'},beforeMs:{type:'integer',minimum:0},afterMs:{type:'integer',minimum:0}
  },['operation']),
  tool('avm_act','Perform one recorded guest input action and return its action ID.',{
    action:{type:'string',enum:['move_pointer','click','double_click','mouse_down','mouse_up','drag','scroll','key_down','key_up','key_press','type_text','wait']},
    x:{type:'integer',minimum:0},y:{type:'integer',minimum:0},
    fromX:{type:'integer',minimum:0},fromY:{type:'integer',minimum:0},toX:{type:'integer',minimum:0},toY:{type:'integer',minimum:0},
    button:{type:'string',enum:['left','middle','right']},
    keycode:{type:'integer',minimum:0,maximum:767},text:{type:'string',minLength:1,maxLength:4096},
    deltaY:{type:'integer',minimum:-100,maximum:100},steps:{type:'integer',minimum:2,maximum:1000},
    durationMs:{type:'integer',minimum:0,maximum:60000},intervalMs:{type:'integer',minimum:0,maximum:1000}
  },['action']),
  tool('avm_click','Move the real guest pointer and click.',{x:{type:'integer',minimum:0},y:{type:'integer',minimum:0}},['x','y']),
  tool('avm_type','Type text through the real guest keyboard.',{text:{type:'string',maxLength:4096}},['text']),
  tool('avm_key','Send one Linux input keycode through QEMU.',{keycode:{type:'integer',minimum:0,maximum:767},mode:{type:'string',enum:['press','down','up']}},['keycode']),
  tool('avm_history','Read recorded cross-source history.',{lastDurationMs:{type:'integer',minimum:1},source:{type:'array',items:{type:'string'}}}),
  tool('avm_query','Query recorded experience. Kinds: aroundEvent, networkFrames, visibleWhilePointerDown, browserElementUnderPointer, evidenceSinceFingerprint, beforeConsoleException, lastDialog, richerVisualEvidence, runtimeTrace.',{query:{type:'object',properties:{kind:{type:'string',enum:['aroundEvent','networkFrames','visibleWhilePointerDown','browserElementUnderPointer','evidenceSinceFingerprint','beforeConsoleException','lastDialog','richerVisualEvidence','runtimeTrace']}},required:['kind'],additionalProperties:true}},['query']),
  tool('avm_accessibility','Observe a fresh native accessibility tree and events.',{durationMs:{type:'integer',minimum:1,maximum:60000}}),
  tool('avm_browser_observe','Record a CDP browser snapshot, network, console, performance, screenshot, and trace.',{durationMs:{type:'integer',minimum:1,maximum:60000}})
];
if(config.localAvm&&config.remoteChannel)tools.push(tool('avm_publish','Publish the current fingerprinted local candidate through the fixed AVM remote channel.',{}));

const lines=readline.createInterface({input:process.stdin,crlfDelay:Infinity});
for await(const line of lines){if(!line.trim())continue;let request;try{request=JSON.parse(line)}catch{continue}if(request.id===undefined)continue;try{let result;if(request.method==='initialize')result={protocolVersion:request.params?.protocolVersion||'2025-06-18',capabilities:{tools:{listChanged:false}},serverInfo:{name:'avm-workstation',version:'1'}};else if(request.method==='tools/list')result={tools};else if(request.method==='tools/call')result=await callTool(request.params?.name,request.params?.arguments||{});else throw new Error(`unsupported method ${request.method}`);send({jsonrpc:'2.0',id:request.id,result})}catch(error){send({jsonrpc:'2.0',id:request.id,result:{content:[{type:'text',text:error.message}],isError:true}})}}

function tool(name,description,properties,required=[]){return{name,description,inputSchema:{type:'object',properties,required,additionalProperties:false}}}
function send(value){process.stdout.write(`${JSON.stringify(value)}\n`)}
function validateConfig(value){for(const key of ['project','zone','instance','remoteAvm','remoteRun'])if(typeof value[key]!=='string'||!value[key])throw new Error(`config missing ${key}`);for(const key of ['project','zone','instance'])if(!/^[a-zA-Z0-9._-]+$/.test(value[key]))throw new Error(`unsafe ${key}`);for(const key of ['remoteAvm','remoteRun'])if(!value[key].startsWith('/')||/[\n\r\0]/.test(value[key]))throw new Error(`unsafe ${key}`)}
function shellQuote(value){return `'${String(value).replaceAll("'","'\\''")}'`}
async function callTool(name,args){
  if(name==='avm_capture')return capture();
  if(name==='avm_experience')return experience(args);
  if(name==='avm_act')return text(await act(args));
  if(name==='avm_click')return text(await runAvm(['act-click','--run',config.remoteRun,'--x',integer(args.x),'--y',integer(args.y)]));
  if(name==='avm_type')return text(await runAvm(['act-type','--run',config.remoteRun,'--text',String(args.text)]));
  if(name==='avm_key')return text(await runAvm(['act-key','--run',config.remoteRun,'--keycode',integer(args.keycode),'--mode',args.mode||'press']));
  if(name==='avm_history'){const command=['history','--run',config.remoteRun,'--last-duration-ms',integer(args.lastDurationMs||10000)];for(const source of args.source||[])command.push('--source',String(source));return text(await runAvm(command))}
  if(name==='avm_query')return text(await query(args.query));
  if(name==='avm_accessibility')return text(await runAvm(['accessibility-observe','--run',config.remoteRun,'--duration-ms',integer(args.durationMs||5000)]));
  if(name==='avm_browser_observe')return text(await runAvm(['browser-observe','--run',config.remoteRun,'--endpoint',config.browserEndpoint||'http://127.0.0.1:9222','--script',config.remoteBrowserScript||'/home/artpar/avm/supervisor/browser/observer.mjs','--duration-ms',integer(args.durationMs||5000)]));
  if(name==='avm_publish'){if(!config.localAvm?.startsWith('/')||!config.remoteChannel?.startsWith('/'))throw new Error('publish channel is not configured');return text(await run(config.localAvm,['remote-publish','--channel',config.remoteChannel]))}
  throw new Error(`unknown AVM tool ${name}`)
}
async function act(args){
  const action=String(args.action||'');const button=args.button||'left';
  if(action==='move_pointer')return runAvm(['act-pointer','--run',config.remoteRun,'--x',requiredInteger(args,'x'),'--y',requiredInteger(args,'y')]);
  if(action==='click')return runAvm(['act-click','--run',config.remoteRun,'--x',requiredInteger(args,'x'),'--y',requiredInteger(args,'y'),'--button',button,'--wait-after-ms',integer(args.durationMs??0)]);
  if(action==='double_click')return runAvm(['act-double-click','--run',config.remoteRun,'--x',requiredInteger(args,'x'),'--y',requiredInteger(args,'y'),'--button',button,'--interval-ms',integer(args.intervalMs??100)]);
  if(action==='mouse_down'||action==='mouse_up')return runAvm(['act-button','--run',config.remoteRun,'--button',button,'--mode',action==='mouse_down'?'down':'up']);
  if(action==='drag')return runAvm(['act-drag','--run',config.remoteRun,'--from-x',requiredInteger(args,'fromX'),'--from-y',requiredInteger(args,'fromY'),'--to-x',requiredInteger(args,'toX'),'--to-y',requiredInteger(args,'toY'),'--button',button,'--steps',integer(args.steps??12),'--duration-ms',integer(args.durationMs??500)]);
  if(action==='scroll')return runAvm(['act-scroll','--run',config.remoteRun,'--delta-y',requiredInteger(args,'deltaY')]);
  if(action==='key_down'||action==='key_up'||action==='key_press')return runAvm(['act-key','--run',config.remoteRun,'--keycode',requiredInteger(args,'keycode'),'--mode',action.slice(4)]);
  if(action==='type_text'){if(typeof args.text!=='string'||!args.text)throw new Error('type_text requires non-empty text');return runAvm(['act-type','--run',config.remoteRun,'--text',args.text])}
  if(action==='wait')return runAvm(['act-wait','--run',config.remoteRun,'--duration-ms',integer(args.durationMs??1000)]);
  throw new Error(`unsupported action ${action}`)
}
function integer(value){if(!Number.isSafeInteger(Number(value)))throw new Error('expected integer');return String(value)}
function requiredInteger(args,name){if(args[name]===undefined)throw new Error(`${args.action} requires ${name}`);return integer(args[name])}
function text(output){return{content:[{type:'text',text:output}]}}
async function experience(args){
  if(args.operation==='observe')return text(await runAvm(['observe','--run',config.remoteRun,'--recent-limit',integer(args.recentLimit??20)]));
  if(args.operation==='frame'){const command=['frame','--run',config.remoteRun];if(args.atNs!==undefined)command.push('--at-ns',integer(args.atNs));if(args.eventId!==undefined)command.push('--event-id',String(args.eventId));return imageFromAvm(command,'Historical frame reconstructed from immutable display evidence.','frame')}
  if(args.operation==='replay'){const command=['replay','--run',config.remoteRun];appendInterval(command,args);return text(await runAvm(command))}
  if(args.operation==='inspect'){
    let queryValue;
    if(args.inspectKind==='event'){if(!args.eventId)throw new Error('event inspection requires eventId');queryValue={kind:'aroundEvent',eventId:String(args.eventId),beforeMs:Number(args.beforeMs??500),afterMs:Number(args.afterMs??2000)}}
    else if(args.inspectKind==='browser_element'){if(!args.eventId)throw new Error('browser element inspection requires a pointer eventId');queryValue={kind:'browserElementUnderPointer',eventId:String(args.eventId)}}
    else if(args.inspectKind==='last_dialog')queryValue={kind:'lastDialog',text:args.text===undefined?null:String(args.text)};
    else throw new Error('inspect requires inspectKind');
    return text(await query(queryValue));
  }
  throw new Error(`unsupported experience operation ${args.operation}`)
}
function appendInterval(command,args){if(args.startNs!==undefined)command.push('--start-ns',integer(args.startNs));if(args.endNs!==undefined)command.push('--end-ns',integer(args.endNs));if(args.lastDurationMs!==undefined)command.push('--last-duration-ms',integer(args.lastDurationMs))}
async function runAvm(args){const command=`cd ${shellQuote(dirname(config.remoteAvm))} && ${[config.remoteAvm,...args].map(shellQuote).join(' ')}`;return run('gcloud',['compute','ssh',config.instance,'--project',config.project,'--zone',config.zone,'--command',command])}
async function capture(){return imageFromAvm(['capture','--run',config.remoteRun],'Authoritative QEMU framebuffer capture.','capture')}
async function imageFromAvm(command,description,label){const remote=`/tmp/avm-mcp-${process.pid}-${Date.now()}.png`;const metadata=await runAvm([...command,'--output',remote]);const temporary=await mkdtemp(join(tmpdir(),`avm-${label}-`));const local=join(temporary,'frame.png');try{await run('gcloud',['compute','scp',`${config.instance}:${remote}`,local,'--project',config.project,'--zone',config.zone]);const data=await readFile(local);return{content:[{type:'image',data:data.toString('base64'),mimeType:'image/png'},{type:'text',text:`${description}\n${metadata}`}]} }finally{await rm(temporary,{recursive:true,force:true})}}
async function query(value){const temporary=await mkdtemp(join(tmpdir(),'avm-query-'));const local=join(temporary,'query.json');const remote=`/tmp/avm-query-${process.pid}-${Date.now()}.json`;try{await writeFile(local,JSON.stringify(value));await run('gcloud',['compute','scp',local,`${config.instance}:${remote}`,'--project',config.project,'--zone',config.zone]);return runAvm(['experience-query','--run',config.remoteRun,'--input',remote])}finally{await rm(temporary,{recursive:true,force:true})}}
function run(program,args){return new Promise((resolveRun,reject)=>{const child=spawn(program,args,{stdio:['ignore','pipe','pipe']});let stdout='',stderr='';child.stdout.on('data',chunk=>stdout+=chunk);child.stderr.on('data',chunk=>stderr+=chunk);child.on('error',reject);child.on('exit',code=>code===0?resolveRun(stdout.trim()):reject(new Error(`${program} exited ${code}: ${stderr.trim()}`)))})}
