# Codex protocol contract

`schema/` is generated from Codex CLI 0.146.0. It is evaluator-owned protocol input, not candidate code.

Regenerate it only when intentionally upgrading the CLI:

```sh
rm -rf supervisor/codex/schema
codex app-server generate-json-schema --out supervisor/codex/schema
codex --version
```

Review changes to `v2/ThreadStartParams.json`, `v2/TurnStartParams.json`, `ServerRequest.json`, and the approval response schemas before updating the supervisor adapter.
