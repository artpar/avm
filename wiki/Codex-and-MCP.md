# Codex and MCP

Codex runs locally and reaches AVM through a fixed, inspectable channel. It does
not authenticate on the GCE host or nested guest.

## Prerequisites

- `gcloud auth` is already active on the local workstation.
- The GCE host is reachable with `gcloud compute ssh` using an explicit project,
  zone, and instance.
- AVM and `supervisor/browser/observer.mjs` exist on the host.
- A run is active, and any required guest application/CDP tunnels are ready.

## Create and publish through a channel

```sh
CHANNEL=$(avm remote-channel-create \
  --local-candidate /absolute/path/to/candidate \
  --state-root /absolute/path/outside/candidate/supervisor \
  --project PROJECT \
  --zone ZONE \
  --instance INSTANCE \
  --remote-run /var/lib/avm/runs/RUN_ID/run.json \
  --remote-avm /home/USER/avm/target/release/avm)

avm remote-publish --channel "$CHANNEL"
```

The channel fixes its remote endpoint. Publishing transfers only the
fingerprinted candidate and records the result into the remote timeline.

## MCP configuration

Store configuration outside the candidate:

```json
{
  "project": "PROJECT",
  "zone": "ZONE",
  "instance": "INSTANCE",
  "remoteAvm": "/home/USER/avm/target/release/avm",
  "remoteRun": "/var/lib/avm/runs/RUN_ID/run.json",
  "remoteBrowserScript": "/home/USER/avm/supervisor/browser/observer.mjs",
  "browserEndpoint": "http://127.0.0.1:9223",
  "localAvm": "/absolute/path/to/avm",
  "remoteChannel": "/absolute/path/to/channel.json"
}
```

Validate the MCP protocol and allowlist without contacting GCE:

```sh
node supervisor/mcp/check.mjs
```

Start local Codex:

```sh
codex exec --ephemeral --sandbox workspace-write \
  -c 'mcp_servers.avm.command="node"' \
  -c 'mcp_servers.avm.args=["/absolute/path/to/supervisor/mcp/avm-server.mjs","/absolute/path/to/avm-mcp.json"]' \
  -c 'mcp_servers.avm.required=true' \
  'Operate the application, diagnose the reported behavior, fix it, and verify it.'
```

## Exposed tool surface

The MCP server provides capture; current/historical experience; recorded
pointer, mouse, keyboard, text, and wait actions; source-filtered history; typed
cross-source queries; AT-SPI accessibility; browser observation; and optional
fingerprinted publication. It has no arbitrary-shell tool.

For a browser-correlated action, call `avm_browser_observe` in `start` mode,
perform `avm_act`, then call the observer in `finish` mode with its observation
ID. This keeps CDP recording active across the real guest input.

## Tunnels

AVM's experiment harness uses two scoped paths through the GCE host:

- a guest application tunnel, typically guest `127.0.0.1:3000` to a host
  loopback port; and
- a browser CDP tunnel, guest `127.0.0.1:9222` to host/local loopback `9223`.

Do not expose CDP publicly. Bind tunnels to loopback, verify readiness before
starting the agent, and remove them during cleanup.
