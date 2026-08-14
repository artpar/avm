import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const [trialArgument, scoreArgument] = process.argv.slice(2);
if (!trialArgument || !scoreArgument) {
  throw new Error('usage: node final-demo-audit.mjs TRIAL_ROOT SCORE_JSON');
}

const trial = resolve(trialArgument);
const score = JSON.parse(await readFile(resolve(scoreArgument), 'utf8'));
const lines = (await readFile(`${trial}/codex-events.jsonl`, 'utf8'))
  .trim().split('\n').filter(Boolean).map(JSON.parse);
const history = JSON.parse(await readFile(`${trial}/remote-history.json`, 'utf8'));
const argumentsById = new Map(lines
  .filter(event => event.type === 'item.started' && event.item?.type === 'mcp_tool_call')
  .map(event => [event.item.id, event.item.arguments ?? {}]));
const completed = lines
  .filter(event => event.type === 'item.completed' && event.item?.type === 'mcp_tool_call' && event.item.status === 'completed')
  .map((event, index) => ({ index, tool: event.item.tool, arguments: argumentsById.get(event.item.id) ?? {} }));
const publishedAt = completed.findIndex(call => call.tool === 'avm_publish');
const before = publishedAt < 0 ? [] : completed.slice(0, publishedAt);
const after = publishedAt < 0 ? [] : completed.slice(publishedAt + 1);
const has = (calls, tool, predicate = () => true) => calls.some(call => call.tool === tool && predicate(call.arguments));
const checks = {
  independentEvaluator: score.functionalDefects === 0 && score.regressions === 0 && score.detail?.checkExitCode === 0 && score.detail?.duplicateCount === 1,
  canonicalTimeline: Array.isArray(history.events) && history.events.length > 0,
  diagnosisInputs: has(before, 'avm_browser_observe') && has(before, 'avm_capture') && has(before, 'avm_act'),
  temporalRevisit: has(before, 'avm_history') && has(before, 'avm_query'),
  replayOrInspection: has(before, 'avm_experience', args => ['replay', 'inspect'].includes(args.operation)),
  published: publishedAt >= 0,
  repeatedGraphicalAction: has(after, 'avm_act') && has(after, 'avm_capture'),
  correctedBrowserEvidence: has(after, 'avm_browser_observe'),
};
const failures = Object.entries(checks).filter(([, passed]) => !passed).map(([name]) => name);
const result = {
  accepted: failures.length === 0,
  checks,
  failures,
  completedTools: completed.map(call => call.tool),
  timelineEvents: history.events?.length ?? 0,
  score,
};
process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
if (failures.length) process.exitCode = 1;
