import assert from 'node:assert/strict';
import { metricsFromAppEvents, metricsFromExecEvents, usageTokens } from './agent-metrics.mjs';

const exec = metricsFromExecEvents([
  { type: 'item.started', item: { type: 'command_execution' } },
  { type: 'item.completed', item: { type: 'command_execution', exit_code: 1 } },
  { type: 'item.completed', item: { type: 'file_change' } },
  { type: 'item.completed', item: { type: 'command_execution', exit_code: 1 } },
  { type: 'item.completed', item: { type: 'file_change' } },
  { type: 'turn.completed', usage: { input_tokens: 80, output_tokens: 20, total_tokens: 100 } },
]);
assert.deepEqual(exec, { toolCalls: 1, modelTokens: 100, failedAttempts: 1, rework: 1, toolFailures: 0 });

const app = metricsFromAppEvents([
  appEvent('codex.command.started', {}),
  appEvent('codex.file_change.completed', { type: 'fileChange', status: 'completed' }),
  appEvent('codex.mcp_tool_call.completed', { type: 'mcpToolCall', status: 'failed' }),
  appEvent('codex.command.completed', { type: 'commandExecution', status: 'failed' }),
  appEvent('codex.file_change.completed', { type: 'fileChange', status: 'completed' }),
  { kind: 'codex.thread.tokenUsage.updated', payload: { message: { params: { tokenUsage: { total: { inputTokens: 110, outputTokens: 12, totalTokens: 122 } } } } } },
]);
assert.deepEqual(app, { toolCalls: 1, modelTokens: 122, failedAttempts: 1, rework: 1, toolFailures: 1 });
assert.equal(usageTokens({ inputTokens: 10, outputTokens: 3, reasoningOutputTokens: 2 }), 13);
console.log(JSON.stringify({ ok: true, formats: ['exec', 'app-server'] }));

function appEvent(kind, item) {
  return { kind, payload: { message: { params: { item } } } };
}
