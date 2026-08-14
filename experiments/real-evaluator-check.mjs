import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawn } from 'node:child_process';
import assert from 'node:assert/strict';

const trial = await mkdtemp(join(tmpdir(), 'avm-real-evaluator-'));
try {
  await writeFile(join(trial, 'agent-metrics.json'), JSON.stringify({ durationMs: 123, toolCalls: 7, modelTokens: 456, productInteractions: 9, failedAttempts: 2, rework: 1 }));
  const child = spawn(process.execPath, ['experiments/real-evaluator.mjs', resolve('benchmarks/target-app'), trial, resolve('benchmarks/evaluator/score-retry-duplicate.mjs')], { stdio: ['ignore', 'pipe', 'inherit'] });
  let stdout = ''; child.stdout.on('data', chunk => stdout += chunk);
  await new Promise((resolveExit, reject) => child.on('exit', code => code === 0 ? resolveExit() : reject(new Error(`evaluator exited ${code}`))));
  const result = JSON.parse(stdout); assert.equal(result.timeMs, 123); assert.equal(result.toolCalls, 7); assert.equal(result.modelTokens, 456); assert.equal(result.productInteractions, 9); assert.equal(typeof result.functionalDefects, 'number');
  assert.equal(result.failedAttempts, 2); assert.equal(result.rework, 1);
  console.log(JSON.stringify({ ok: true, mergedOperationalMetrics: true }));
} finally { await rm(trial, { recursive: true, force: true }); }
