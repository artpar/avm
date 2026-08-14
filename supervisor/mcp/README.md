# AVM workstation MCP server

`avm-server.mjs` runs locally beside Codex. It exposes eight narrow tools for authoritative capture, recorded input, history, structured queries, AT-SPI observation, and CDP observation. Each call uses `gcloud compute ssh` or `scp` with an explicit project, zone, instance, remote AVM binary, and run config. It has no arbitrary-shell tool, and candidate code receives neither its config nor Codex/GCE credentials.

Example Codex CLI overrides:

```sh
codex exec --json --ephemeral --sandbox workspace-write \
  -c 'mcp_servers.avm.command="node"' \
  -c 'mcp_servers.avm.args=["/path/to/supervisor/mcp/avm-server.mjs","/outside/candidate/avm-mcp.json"]' \
  -c 'mcp_servers.avm.required=true' \
  -c 'mcp_servers.avm.default_tools_approval_mode="approve"' \
  'Operate the running application, diagnose the reported behavior, and fix it.'
```

The config and server must remain outside the candidate workspace. Codex authenticates locally; nothing runs `codex login` in the GCE host or nested guest. Run `node supervisor/mcp/check.mjs` to verify protocol initialization and the exact tool allowlist without contacting GCE.
