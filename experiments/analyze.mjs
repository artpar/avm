import { readFile } from 'node:fs/promises';

const [resultsPath] = process.argv.slice(2);
if (!resultsPath) throw new Error('usage: node analyze.mjs RESULTS_JSONL');
const rows = (await readFile(resultsPath, 'utf8')).trim().split('\n').filter(Boolean).map(JSON.parse);
const conditions = Object.fromEntries(['A', 'B', 'C', 'D'].map(condition => [condition, rows.filter(row => row.condition === condition)]));
if (rows.length < 8 || Object.values(conditions).some(group => group.length < 2)) throw new Error('results must contain at least two trials for each condition');
if (rows.some(row => row.agent.exitCode !== 0 || row.agent.timedOut || row.evaluator.exitCode !== 0 || row.evaluator.timedOut)) throw new Error('results contain a failed or timed-out trial');

const metrics = ['functionalDefects', 'hiddenDefectsDiscovered', 'userFacingDefects', 'temporalDefects', 'incorrectRequirementInterpretations', 'regressions', 'timeMs', 'toolCalls', 'modelTokens', 'productInteractions', 'diagnosisAccuracy', 'humanInterventions', 'catastrophicImplementations'];
const summarize = group => Object.fromEntries(metrics.map(metric => {
  const values = group.map(row => row.metrics[metric]).filter(Number.isFinite);
  return [metric, { mean: values.length ? values.reduce((sum, value) => sum + value, 0) / values.length : null, available: values.length }];
}));
const conditionSummary = Object.fromEntries(Object.entries(conditions).map(([condition, group]) => [condition, { n: group.length, metrics: summarize(group) }]));
const grouped = names => names.flatMap(name => conditions[name]);
const difference = (enabled, disabled) => {
  const enabledSummary = summarize(enabled), disabledSummary = summarize(disabled);
  return Object.fromEntries(metrics.map(metric => {
    const left = enabledSummary[metric], right = disabledSummary[metric];
    return [metric, left.available === enabled.length && right.available === disabled.length ? subtract(left.mean, right.mean) : null];
  }));
};
const interaction = Object.fromEntries(metrics.map(metric => {
  const value = name => conditionSummary[name].metrics[metric].mean;
  return [metric, combine(value('B'), value('C'), value('D'), value('A'))];
}));
process.stdout.write(`${JSON.stringify({
  trials: rows.length,
  conditionSummary,
  effects: {
    richPerceptionEnabledMinusDisabled: difference(grouped(['B', 'C']), grouped(['A', 'D'])),
    evidenceGatingEnabledMinusDisabled: difference(grouped(['B', 'D']), grouped(['A', 'C'])),
    interactionBMinusCMinusDPlusA: interaction,
  },
}, null, 2)}\n`);

function subtract(left, right) { return Number.isFinite(left) && Number.isFinite(right) ? left - right : null; }
function combine(b, c, d, a) { return [b, c, d, a].every(Number.isFinite) ? b - c - d + a : null; }
