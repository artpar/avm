import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawn } from 'node:child_process';
import assert from 'node:assert/strict';

const root = await mkdtemp(join(tmpdir(), 'avm-real-agent-'));
const config = join(root, 'config.json');
await writeFile(config, JSON.stringify({
  ...Object.fromEntries([
    'model','codex','localAvm','mcpServer','gcloud','project','zone','instance','remoteAvm','remoteBrowserScript','remoteCandidateRoot','remoteStateRoot','baseImage','guestSshKey','remoteFaultProxy','remoteFaultProfile'
  ].map(key => [key, `/fixture/${key}`])),
  agentWallTimeMs: 1,
  vmWallTimeMs: 1,
}));
try {
  for (const condition of ['A', 'B', 'C', 'D']) {
    const trial = join(root, condition); const workspace = resolve('benchmarks/target-app');
    const child = spawn(process.execPath, ['experiments/real-agent.mjs', config, condition, workspace, trial, 'benchmarks/tasks/retry-duplicate.md', '--dry-run'], { stdio: ['ignore', 'pipe', 'inherit'] });
    let stdout = ''; child.stdout.on('data', chunk => stdout += chunk);
    await new Promise((resolveExit, reject) => child.on('exit', code => code === 0 ? resolveExit() : reject(new Error(`condition ${condition} exited ${code}`))));
    const plan = JSON.parse(stdout); assert.equal(plan.richPerception, condition === 'B' || condition === 'C'); assert.equal(plan.evidenceGating, condition === 'B' || condition === 'D');
    assert.equal(plan.candidateExecution, plan.richPerception ? 'nested-guest' : 'local-tests-only');
    assert.equal(plan.browserTransport, plan.richPerception ? 'guest-loopback-via-ssh-tunnel' : 'not-exposed');
    assert.deepEqual(JSON.parse(await readFile(join(trial, 'capability-plan.json'), 'utf8')), plan);
  }
  console.log(JSON.stringify({ ok: true, conditions: 4, candidateExecution: 'nested-guest-for-rich' }));
} finally { await rm(root, { recursive: true, force: true }); }
