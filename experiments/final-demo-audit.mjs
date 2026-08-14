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
const has = (calls, tool, predicate = () => true) => calls.some(call => call.tool === tool && predicate(call.arguments));
const overlapsAction = calls => {
  const start = calls.findIndex(call => call.tool === 'avm_browser_observe' && call.arguments.mode === 'start');
  if (start < 0) return false;
  const action = calls.findIndex((call, index) => index > start && ['avm_act', 'avm_click', 'avm_type', 'avm_key'].includes(call.tool));
  return action >= 0 && calls.some((call, index) => index > action && call.tool === 'avm_browser_observe' && call.arguments.mode === 'finish');
};
const boundaries = completed.flatMap((call, index) => call.tool === 'avm_publish' ? [{ index, before: completed.slice(0, index), after: completed.slice(index + 1) }] : []);
const boundary = boundaries.find(candidate =>
  overlapsAction(candidate.before) && has(candidate.before, 'avm_capture') && has(candidate.before, 'avm_history') &&
  has(candidate.before, 'avm_query') && has(candidate.before, 'avm_experience', args => ['replay', 'inspect'].includes(args.operation)) &&
  overlapsAction(candidate.after) && has(candidate.after, 'avm_capture'));
const before = boundary?.before ?? [];
const after = boundary?.after ?? [];
const checks = {
  independentEvaluator: score.functionalDefects === 0 && score.regressions === 0 && score.detail?.checkExitCode === 0 && score.detail?.duplicateCount === 1,
  canonicalTimeline: Array.isArray(history.events) && history.events.length > 0,
  diagnosisInputs: overlapsAction(before) && has(before, 'avm_capture'),
  temporalRevisit: has(before, 'avm_history') && has(before, 'avm_query'),
  replayOrInspection: has(before, 'avm_experience', args => ['replay', 'inspect'].includes(args.operation)),
  published: Boolean(boundary),
  repeatedGraphicalAction: overlapsAction(after) && has(after, 'avm_capture'),
  correctedBrowserEvidence: overlapsAction(after),
};
const failures = Object.entries(checks).filter(([, passed]) => !passed).map(([name]) => name);
const result = {
  accepted: failures.length === 0,
  checks,
  failures,
  acceptedPublishOrdinal: boundary ? boundaries.indexOf(boundary) + 1 : null,
  completedTools: completed.map(call => call.tool),
  timelineEvents: history.events?.length ?? 0,
  score,
};
process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
if (failures.length) process.exitCode = 1;
