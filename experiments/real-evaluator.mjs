import { readFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { spawn } from 'node:child_process';

const [workspaceArgument, trialRootArgument, scorerArgument] = process.argv.slice(2);
if (!workspaceArgument || !trialRootArgument || !scorerArgument) throw new Error('usage: node real-evaluator.mjs WORKSPACE TRIAL_ROOT SCORER');
const workspace = resolve(workspaceArgument); const trialRoot = resolve(trialRootArgument); const scorer = resolve(scorerArgument);
const child = spawn(process.execPath, [scorer, workspace], { stdio: ['ignore', 'pipe', 'pipe'], env: { PATH: process.env.PATH, LANG: 'C.UTF-8' } });
let stdout = '', stderr = ''; child.stdout.on('data', chunk => stdout += chunk); child.stderr.on('data', chunk => stderr += chunk);
const exitCode = await new Promise(resolveExit => child.on('exit', resolveExit));
if (exitCode !== 0) throw new Error(`hidden scorer exited ${exitCode}: ${stderr}`);
const score = JSON.parse(stdout.trim());
let agent = {}; try { agent = JSON.parse(await readFile(join(trialRoot, 'agent-metrics.json'), 'utf8')); } catch {}
const merged = {
  ...score,
  timeMs: agent.durationMs ?? score.timeMs,
  toolCalls: agent.toolCalls ?? score.toolCalls,
  modelTokens: agent.modelTokens ?? score.modelTokens,
  productInteractions: agent.productInteractions ?? score.productInteractions,
};
process.stdout.write(`${JSON.stringify(merged)}\n`);
