import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { spawn } from 'node:child_process';

const [configPath, condition, workspaceArgument, trialRootArgument, taskPath, dryRunFlag] = process.argv.slice(2);
if (!configPath || !['A', 'B', 'C', 'D'].includes(condition) || !workspaceArgument || !trialRootArgument || !taskPath) {
  throw new Error('usage: node real-agent.mjs CONFIG A|B|C|D WORKSPACE TRIAL_ROOT TASK [--dry-run]');
}
const config = JSON.parse(await readFile(configPath, 'utf8'));
const workspace = resolve(workspaceArgument);
const trialRoot = resolve(trialRootArgument);
const task = (await readFile(taskPath, 'utf8')).trim();
const rich = condition === 'B' || condition === 'C';
const gated = condition === 'B' || condition === 'D';
const dryRun = dryRunFlag === '--dry-run';
validateConfig(config);
await mkdir(trialRoot, { recursive: true });

const plan = {
  condition,
  richPerception: rich,
  evidenceGating: gated,
  codexLocation: 'local',
  candidateExecution: rich ? 'nested-guest' : 'local-tests-only',
  vmCleanup: rich ? 'stop-qemu-then-stop-gce' : 'not-started',
};
await writeFile(join(trialRoot, 'capability-plan.json'), `${JSON.stringify(plan, null, 2)}\n`);
if (dryRun) {
  process.stdout.write(`${JSON.stringify(plan)}\n`);
  process.exit(0);
}

let remote;
const started = process.hrtime.bigint();
try {
  if (rich) remote = await prepareRemote();
  const prompt = buildPrompt(task, rich, gated);
  const agent = gated ? await runGated(prompt, remote) : await runOrdinary(prompt, remote);
  if (gated) {
    must(await run(config.localAvm, ['evidence-command', '--policy', agent.policy, '--cwd', workspace, '--expected-exit-code', '0', 'npm', 'run', 'check'], workspace, config.agentWallTimeMs));
    if (rich) must(await run(config.localAvm, ['remote-publish', '--channel', remote.channel], workspace, config.agentWallTimeMs));
  }
  const usage = await agentUsage(agent);
  const productInteractions = remote ? await recordedInteractions(remote) : 0;
  const metrics = {
    durationMs: Number(process.hrtime.bigint() - started) / 1e6,
    richPerception: rich,
    evidenceGating: gated,
    exitCode: agent.result.exitCode,
    timedOut: agent.result.timedOut,
    remoteRun: remote?.run ?? null,
    toolCalls: usage.toolCalls,
    modelTokens: usage.modelTokens,
    productInteractions,
  };
  await writeFile(join(trialRoot, 'agent-metrics.json'), `${JSON.stringify(metrics, null, 2)}\n`);
  process.stdout.write(`${JSON.stringify(metrics)}\n`);
  if (agent.result.exitCode !== 0 || agent.result.timedOut) process.exitCode = 1;
} finally {
  if (remote) await cleanupRemote(remote);
}

async function runOrdinary(prompt, remoteState) {
  const args = ['exec', '--json', '--ephemeral', '--ignore-user-config', '--ignore-rules', '--sandbox', 'workspace-write', '--color', 'never', '-m', config.model, '-C', workspace, '-o', join(trialRoot, 'codex-final.txt')];
  if (remoteState) args.push(...mcpOverrides(remoteState.mcp));
  args.push(prompt);
  return { result: await run(config.codex, args, workspace, config.agentWallTimeMs) };
}

async function runGated(prompt, remoteState) {
  const stateRoot = join(trialRoot, 'supervisor');
  const session = output(await run(config.localAvm, ['session-create', '--candidate', workspace, '--state-root', stateRoot], workspace));
  const policyConfig = join(trialRoot, 'policy-config.json');
  await writeFile(policyConfig, `${JSON.stringify({ maximumMutationEventsWithoutEvidence: 1, maximumEvidenceDebt: 6, rules: [] }, null, 2)}\n`);
  const policy = output(await run(config.localAvm, ['policy-init', '--target', session, '--config', policyConfig], workspace));
  const declaration = join(trialRoot, 'declaration.json');
  await writeFile(declaration, `${JSON.stringify({
    intendedBehaviorChange: 'Make one logical card-create action persist exactly one card across network retries.',
    causalHypothesis: 'The create operation is not idempotent across repeated delivery of one logical request.',
    observationType: 'targeted_test',
    instrument: 'evaluator-owned command recorder',
    actionOrCommand: 'npm run check',
    predictedResult: 'The targeted create, move, persistence, and undo checks exit successfully.',
    contradictionMeaning: 'The implementation still breaks required behavior or its retry regression.',
  }, null, 2)}\n`);
  must(await run(config.localAvm, ['policy-declare', '--policy', policy, '--input', declaration], workspace));
  const args = ['codex-turn', '--session', session, '--policy', policy, '--prompt', prompt, '--model', config.model, '--approval', 'decline', '--approval-policy', 'on-request'];
  if (remoteState) {
    for (const value of appServerMcpOverrides(remoteState.mcp)) args.push('--codex-config', value);
  }
  const result = await run(config.localAvm, args, workspace, config.agentWallTimeMs);
  return { result, policy, session };
}

async function agentUsage(agent) {
  if (agent.session) {
    const lines = (await readFile(join(dirname(agent.session), 'events.jsonl'), 'utf8')).trim().split('\n').filter(Boolean).map(JSON.parse);
    const toolCalls = lines.filter(event => event.kind === 'codex.command.started' || event.kind === 'codex.mcp_tool_call.started').length;
    const completed = lines.filter(event => event.kind === 'codex.turn.completed').at(-1)?.payload;
    return { toolCalls, modelTokens: usageTokens(completed) };
  }
  const events = agent.result.stdout.split('\n').filter(Boolean).flatMap(line => { try { return [JSON.parse(line)]; } catch { return []; } });
  const toolCalls = events.filter(event => event.type === 'item.started' && ['command_execution', 'mcp_tool_call'].includes(event.item?.type)).length;
  return { toolCalls, modelTokens: usageTokens(events.filter(event => event.type === 'turn.completed').at(-1)) };
}

async function recordedInteractions(remoteState) {
  const history = await gcloudAvm(['history', '--run', remoteState.run, '--source', 'input']);
  try {
    const value = JSON.parse(history.stdout);
    return value.events.filter(event => event.kind !== 'input.action.completed').length;
  } catch { return null; }
}

function usageTokens(value) {
  const usage = value?.usage || value?.turn?.usage || value?.params?.turn?.usage;
  if (!usage || typeof usage !== 'object') return null;
  const values = Object.entries(usage).filter(([key, item]) => /token/i.test(key) && typeof item === 'number' && !/total/i.test(key)).map(([, item]) => item);
  return values.length ? values.reduce((sum, item) => sum + item, 0) : null;
}

async function prepareRemote() {
  must(await run(config.gcloud, ['compute', 'instances', 'start', config.instance, '--project', config.project, '--zone', config.zone, '--quiet'], workspace, config.vmWallTimeMs));
  const label = trialRoot.split('/').at(-1);
  if (!/^[a-zA-Z0-9._-]+$/.test(label)) throw new Error('unsafe trial label');
  const remoteCandidate = `${config.remoteCandidateRoot}/${label}`;
  const remoteStateRoot = `${config.remoteStateRoot}/${label}`;
  await gcloudSsh(`mkdir -p ${quote(remoteCandidate)} ${quote(remoteStateRoot)}`);
  const runPath = output(await gcloudAvm(['create-run', '--base-image', config.baseImage, '--candidate', remoteCandidate, '--state-root', remoteStateRoot]));
  const localSupervisor = join(trialRoot, 'remote-channel');
  const channel = output(await run(config.localAvm, ['remote-channel-create', '--local-candidate', workspace, '--state-root', localSupervisor, '--project', config.project, '--zone', config.zone, '--instance', config.instance, '--remote-run', runPath, '--remote-avm', config.remoteAvm], workspace));
  must(await run(config.localAvm, ['remote-publish', '--channel', channel], workspace, config.vmWallTimeMs));
  await gcloudAvm(['start', '--run', runPath]);
  await waitForGuest();
  const guestLog = `/tmp/avm-target-${label}.log`;
  const guestState = `/tmp/avm-board-${label}.json`;
  await gcloudSsh(`${guestSsh()} ${quote(`cd /workspace && HOST=127.0.0.1 PORT=3000 BOARD_STATE=${guestState} BOARD_LATENCY_MS=120 nohup node --watch server.mjs >${guestLog} 2>&1 &`)} `);
  const tunnelPid = output(await gcloudSsh(`${guestSsh('-N -L 127.0.0.1:13000:127.0.0.1:3000')} >/tmp/avm-tunnel-${label}.log 2>&1 & echo $!`));
  const proxyPid = output(await gcloudSsh(`cd ${quote(dirname(config.remoteFaultProxy))} && EVALUATOR_PORT=3001 TARGET_ORIGIN=http://127.0.0.1:13000 FAULT_PROFILE=${quote(config.remoteFaultProfile)} nohup node ${quote(config.remoteFaultProxy)} >/tmp/avm-proxy-${label}.log 2>&1 & echo $!`));
  await gcloudAvm(['act-type', '--run', runPath, '--text', 'http://10.0.2.2:3001']);
  await gcloudAvm(['act-key', '--run', runPath, '--keycode', '28', '--mode', 'press']);
  const mcp = join(trialRoot, 'avm-mcp.json');
  await writeFile(mcp, `${JSON.stringify({
    project: config.project, zone: config.zone, instance: config.instance,
    remoteAvm: config.remoteAvm, remoteRun: runPath,
    remoteBrowserScript: config.remoteBrowserScript,
    browserEndpoint: 'http://127.0.0.1:9222',
    ...(condition === 'C' ? { localAvm: config.localAvm, remoteChannel: channel } : {}),
  }, null, 2)}\n`);
  return { run: runPath, channel, mcp, tunnelPid, proxyPid };
}

async function waitForGuest() {
  const command = `for n in $(seq 1 120); do ${guestSsh()} ${quote('test -d /workspace && systemctl is-active --quiet weston.service && pgrep -x chrome >/dev/null')} >/dev/null 2>&1 && exit 0; sleep 1; done; exit 1`;
  await gcloudSsh(command, config.vmWallTimeMs);
}

async function cleanupRemote(remoteState) {
  try { await gcloudAvm(['stop', '--run', remoteState.run]); } catch {}
  try { await gcloudSsh(`kill ${Number(remoteState.proxyPid)} ${Number(remoteState.tunnelPid)} 2>/dev/null || true`); } catch {}
  try { must(await run(config.gcloud, ['compute', 'instances', 'stop', config.instance, '--project', config.project, '--zone', config.zone, '--quiet'], workspace, config.vmWallTimeMs)); } catch {}
}

function buildPrompt(taskText, hasRichPerception, hasGate) {
  const capabilities = hasRichPerception
    ? 'Use the AVM tools to inspect and operate the running product before editing. After a fix, publish if avm_publish is available and verify through the guest.'
    : 'You have ordinary filesystem, shell, and local test tools only; no running-product perception tools are available.';
  const gate = hasGate
    ? 'Your writes are isolated in an externally owned staging workspace and will be promoted as one declared batch; the supervisor will independently record the targeted check afterward.'
    : 'No external evidence gate is active.';
  return `${taskText}\n\n${capabilities}\n${gate}\nDo not request VM shell access or inspect evaluator-private files.`;
}

function mcpOverrides(path) {
  return ['-c', 'mcp_servers.avm.command="node"', '-c', `mcp_servers.avm.args=[${JSON.stringify(config.mcpServer)},${JSON.stringify(path)}]`, '-c', 'mcp_servers.avm.required=true', '-c', 'mcp_servers.avm.default_tools_approval_mode="approve"'];
}
function appServerMcpOverrides(path) {
  return ['mcp_servers.avm.command="node"', `mcp_servers.avm.args=[${JSON.stringify(config.mcpServer)},${JSON.stringify(path)}]`, 'mcp_servers.avm.required=true', 'mcp_servers.avm.default_tools_approval_mode="approve"'];
}
function guestSsh(extra = '') {
  return `ssh -o BatchMode=yes -o ConnectTimeout=3 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -i ${quote(config.guestSshKey)} -p 2222 ${extra} avm@127.0.0.1`;
}
function gcloudAvm(args, timeout = config.vmWallTimeMs) {
  return gcloudSsh(`cd ${quote(dirname(config.remoteAvm))} && ${[config.remoteAvm, ...args].map(quote).join(' ')}`, timeout);
}
async function gcloudSsh(command, timeout = config.vmWallTimeMs) {
  return must(await run(config.gcloud, ['compute', 'ssh', config.instance, '--project', config.project, '--zone', config.zone, '--command', command, '--quiet'], workspace, timeout));
}
function output(result) {
  if (result.exitCode !== 0 || result.timedOut) throw new Error(`command failed: ${result.stderr || result.stdout}`);
  return result.stdout.trim().split('\n').at(-1);
}
function must(result) { output(result); return result; }
function run(program, args, cwd, timeoutMs = config.agentWallTimeMs) {
  return new Promise(resolveRun => {
    const child = spawn(program, args, { cwd, stdio: ['ignore', 'pipe', 'pipe'], env: { PATH: process.env.PATH, LANG: 'C.UTF-8' } });
    let stdout = '', stderr = '', timedOut = false;
    child.stdout.on('data', chunk => stdout += chunk);
    child.stderr.on('data', chunk => stderr += chunk);
    const timer = setTimeout(() => { timedOut = true; child.kill('SIGTERM'); }, timeoutMs);
    child.on('error', error => { clearTimeout(timer); resolveRun({ exitCode: null, timedOut, stdout, stderr: `${stderr}${error.message}` }); });
    child.on('exit', exitCode => { clearTimeout(timer); resolveRun({ exitCode, timedOut, stdout, stderr }); });
  });
}
function quote(value) { return `'${String(value).replaceAll("'", "'\\''")}'`; }
function validateConfig(value) {
  for (const key of ['model', 'codex', 'localAvm', 'mcpServer', 'gcloud', 'project', 'zone', 'instance', 'remoteAvm', 'remoteBrowserScript', 'remoteCandidateRoot', 'remoteStateRoot', 'baseImage', 'guestSshKey', 'remoteFaultProxy', 'remoteFaultProfile', 'agentWallTimeMs', 'vmWallTimeMs']) {
    if (value[key] === undefined || value[key] === '') throw new Error(`config missing ${key}`);
  }
}
