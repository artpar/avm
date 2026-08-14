import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';

const root = await mkdtemp(join(tmpdir(), 'avm-final-demo-audit-'));
try {
  const tools = [
    ['avm_browser_observe', { mode: 'start' }], ['avm_capture', {}], ['avm_act', { action: 'click' }], ['avm_browser_observe', { mode: 'finish', observationId: 'before' }],
    ['avm_history', {}], ['avm_query', { query: { kind: 'aroundEvent', eventId: 'event-1' } }],
    ['avm_experience', { operation: 'replay' }], ['avm_publish', {}],
    ['avm_browser_observe', { mode: 'start' }], ['avm_act', { action: 'click' }], ['avm_capture', {}], ['avm_browser_observe', { mode: 'finish', observationId: 'after' }],
  ];
  const events = tools.flatMap(([tool, argumentsValue], index) => [
    { type: 'item.started', item: { id: `item-${index}`, type: 'mcp_tool_call', tool, arguments: argumentsValue } },
    { type: 'item.completed', item: { id: `item-${index}`, type: 'mcp_tool_call', tool, status: 'completed' } },
  ]);
  await writeFile(join(root, 'codex-events.jsonl'), `${events.map(JSON.stringify).join('\n')}\n`);
  await writeFile(join(root, 'remote-history.json'), JSON.stringify({ events: [{ id: 'event-1' }] }));
  const passingScore = join(root, 'passing.json');
  await writeFile(passingScore, JSON.stringify({ functionalDefects: 0, regressions: 0, detail: { checkExitCode: 0, duplicateCount: 1 } }));
  const accepted = await audit(passingScore);
  if (accepted.exitCode !== 0 || !JSON.parse(accepted.stdout).accepted) throw new Error(`valid demo rejected: ${accepted.stderr || accepted.stdout}`);

  const failingScore = join(root, 'failing.json');
  await writeFile(failingScore, JSON.stringify({ functionalDefects: 1, regressions: 0, detail: { checkExitCode: 0, duplicateCount: 2 } }));
  const rejected = await audit(failingScore);
  if (rejected.exitCode === 0 || JSON.parse(rejected.stdout).accepted) throw new Error('defective demo accepted');
  process.stdout.write('final demo audit checks passed\n');
} finally {
  await rm(root, { recursive: true, force: true });
}

function audit(score) {
  return new Promise(resolveAudit => {
    const child = spawn(process.execPath, ['experiments/final-demo-audit.mjs', root, score], { stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '', stderr = '';
    child.stdout.on('data', chunk => stdout += chunk);
    child.stderr.on('data', chunk => stderr += chunk);
    child.on('exit', exitCode => resolveAudit({ exitCode, stdout, stderr }));
  });
}
