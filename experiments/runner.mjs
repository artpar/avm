import { cp, mkdir, readFile, writeFile, appendFile } from 'node:fs/promises';
import { resolve, join, dirname } from 'node:path';
import { spawn } from 'node:child_process';
import crypto from 'node:crypto';

const [manifestPath, outputRoot] = process.argv.slice(2);
if (!manifestPath || !outputRoot) throw new Error('usage: node runner.mjs MANIFEST OUTPUT_ROOT');
const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
const repositoryRoot=resolve(dirname(resolve(manifestPath)),'..');
validate(manifest);
const output = resolve(outputRoot); await mkdir(output,{recursive:true});
await writeFile(join(output,'manifest.json'),`${JSON.stringify(manifest,null,2)}\n`,{flag:'wx'});
const schedule = buildSchedule(manifest); await writeFile(join(output,'schedule.json'),`${JSON.stringify(schedule,null,2)}\n`,{flag:'wx'});

for (const trial of schedule) {
  const trialRoot=join(output,'trials',trial.blindLabel); const workspace=join(trialRoot,'workspace'); await mkdir(trialRoot,{recursive:true});
  await cp(resolve(manifest.repository),workspace,{recursive:true});
  const substitutions={workspace,trialRoot,task:resolve(manifest.task),condition:trial.condition,blindLabel:trial.blindLabel,repositoryRoot};
  const started=process.hrtime.bigint(); const agent=await run(expand(manifest.conditions[trial.condition].agentCommand,substitutions),workspace,manifest.resourceAllowance.wallTimeMs);
  const evaluator=await run(expand(manifest.evaluatorCommand,substitutions),trialRoot,manifest.resourceAllowance.evaluatorWallTimeMs);
  const ended=process.hrtime.bigint();
  let metrics={}; try { metrics=JSON.parse(evaluator.stdout.trim()||'{}'); } catch { metrics={evaluatorParseError:true}; }
  const record={blindLabel:trial.blindLabel,repetition:trial.repetition,condition:trial.condition,capabilities:manifest.conditions[trial.condition].capabilities,model:manifest.model,modelSettings:manifest.modelSettings,durationMs:Number(ended-started)/1e6,agent,evaluator:{...evaluator,stdout:undefined,stderr:evaluator.stderr},metrics};
  await writeFile(join(trialRoot,'result.json'),`${JSON.stringify(record,null,2)}\n`);
  await appendFile(join(output,'results.jsonl'),`${JSON.stringify(record)}\n`);
  const failed=agent.exitCode!==0||agent.timedOut||evaluator.exitCode!==0||evaluator.timedOut;
  if(failed){process.stdout.write(`${JSON.stringify({blindLabel:trial.blindLabel,repetition:trial.repetition,completed:false})}\n`);throw new Error(`trial ${trial.blindLabel} failed`);}
  process.stdout.write(`${JSON.stringify({blindLabel:trial.blindLabel,repetition:trial.repetition,completed:true})}\n`);
}

function validate(value) {
  for(const key of ['model','modelSettings','repository','task','startingVmSnapshot','dependenciesDigest','resourceAllowance','evaluatorCommand','conditions','repetitions']) if(value[key]===undefined) throw new Error(`manifest missing ${key}`);
  if(!Number.isInteger(value.repetitions)||value.repetitions<2) throw new Error('repetitions must be at least two');
  const expected={A:[false,false],B:[true,true],C:[true,false],D:[false,true]};
  if(Object.keys(value.conditions).sort().join('')!=='ABCD') throw new Error('conditions must be exactly A, B, C, and D');
  for(const [name,[richPerception,evidenceGating]] of Object.entries(expected)) {
    const condition=value.conditions[name]; if(!Array.isArray(condition.agentCommand)||!condition.agentCommand.length) throw new Error(`condition ${name} has no direct agent command`);
    if(condition.capabilities?.richPerception!==richPerception||condition.capabilities?.evidenceGating!==evidenceGating) throw new Error(`condition ${name} capability assignment is invalid`);
  }
  if(!Array.isArray(value.evaluatorCommand)||!value.evaluatorCommand.length) throw new Error('evaluatorCommand must be a direct command array');
  if(!value.resourceAllowance.wallTimeMs||!value.resourceAllowance.evaluatorWallTimeMs) throw new Error('resource wall-time limits are required');
}

function buildSchedule(value) {
  const items=[]; for(let repetition=1;repetition<=value.repetitions;repetition++) for(const condition of ['A','B','C','D']) items.push({condition,repetition,blindLabel:crypto.createHash('sha256').update(`${value.randomizationSeed}:${repetition}:${condition}`).digest('hex').slice(0,12)});
  let state=crypto.createHash('sha256').update(String(value.randomizationSeed)).digest().readUInt32LE(0);
  for(let index=items.length-1;index>0;index--){state=(1664525*state+1013904223)>>>0;const swap=state%(index+1);[items[index],items[swap]]=[items[swap],items[index]];}
  return items;
}

function expand(command,values){return command.map(part=>part.replaceAll(/\{(workspace|trialRoot|task|condition|blindLabel|repositoryRoot)\}/g,(_,key)=>values[key]));}

function run(command,cwd,timeoutMs){return new Promise(resolve=>{const started=process.hrtime.bigint();const child=spawn(command[0],command.slice(1),{cwd,stdio:['ignore','pipe','pipe'],env:{PATH:process.env.PATH,LANG:'C.UTF-8'}});let stdout='';let stderr='';let timedOut=false;child.stdout.on('data',chunk=>stdout+=chunk);child.stderr.on('data',chunk=>stderr+=chunk);const timer=setTimeout(()=>{timedOut=true;child.kill('SIGTERM')},timeoutMs);child.on('error',error=>{clearTimeout(timer);resolve({exitCode:null,signal:null,timedOut,durationMs:Number(process.hrtime.bigint()-started)/1e6,stdout,stderr:`${stderr}${error.message}`})});child.on('exit',(exitCode,signal)=>{clearTimeout(timer);resolve({exitCode,signal,timedOut,durationMs:Number(process.hrtime.bigint()-started)/1e6,stdout,stderr})})})}
