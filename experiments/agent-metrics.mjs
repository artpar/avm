export function metricsFromExecEvents(events) {
  const items = events
    .filter(event => event.type === 'item.completed')
    .map(event => event.item || {});
  const usage = events.filter(event => event.type === 'turn.completed').at(-1)?.usage;
  return {
    toolCalls: events.filter(event => event.type === 'item.started' && ['command_execution', 'mcp_tool_call'].includes(event.item?.type)).length,
    modelTokens: usageTokens(usage),
    ...implementationAttemptMetrics(items),
  };
}

export function metricsFromAppEvents(events) {
  const items = events
    .filter(event => ['codex.command.completed', 'codex.mcp_tool_call.completed', 'codex.file_change.completed'].includes(event.kind))
    .map(event => event.payload?.message?.params?.item || {});
  const usage = events
    .filter(event => event.kind === 'codex.thread.tokenUsage.updated')
    .at(-1)?.payload?.message?.params?.tokenUsage?.total;
  return {
    toolCalls: events.filter(event => event.kind === 'codex.command.started' || event.kind === 'codex.mcp_tool_call.started').length,
    modelTokens: usageTokens(usage),
    ...implementationAttemptMetrics(items),
  };
}

export function usageTokens(usage) {
  if (!usage || typeof usage !== 'object') return null;
  const total = usage.totalTokens ?? usage.total_tokens;
  if (typeof total === 'number') return total;
  const input = usage.inputTokens ?? usage.input_tokens;
  const output = usage.outputTokens ?? usage.output_tokens;
  return typeof input === 'number' && typeof output === 'number' ? input + output : null;
}

function implementationAttemptMetrics(items) {
  let mutationSeen = false;
  let failedAfterMutation = false;
  let failedAttempts = 0;
  let rework = 0;
  let toolFailures = 0;
  for (const item of items) {
    const type = item.type;
    if (type === 'file_change' || type === 'fileChange') {
      if (failedAfterMutation) rework += 1;
      mutationSeen = true;
      continue;
    }
    const failed = item.status === 'failed'
      || (typeof item.exit_code === 'number' && item.exit_code !== 0)
      || (typeof item.exitCode === 'number' && item.exitCode !== 0);
    if (!failed) continue;
    if (type === 'mcp_tool_call' || type === 'mcpToolCall') toolFailures += 1;
    if (mutationSeen && (type === 'command_execution' || type === 'commandExecution')) {
      failedAttempts += 1;
      failedAfterMutation = true;
    }
  }
  return { failedAttempts, rework, toolFailures };
}
