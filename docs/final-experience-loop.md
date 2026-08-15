# Final end-to-end experience loop

The remaining completion gate passed in one uninterrupted condition-C Codex
trajectory. Codex stayed on the local supervisor machine; the application ran
inside the nested QEMU/KVM guest on GCE. The accepted evidence directory is:

`/Users/artpar/workspace/code/avm-evidence/final-experience-loop/run-3`

The candidate began at pinned baseline commit `63424d8`. The agent received the
retry-duplicate task, used a ready-acknowledged CDP observation around real QEMU
keyboard input, waited 1.8 seconds, read canonical history, captured the
framebuffer, and successfully queried `aroundEvent` with canonical event UUID
`1287cd50-6655-43c6-a650-35e6ee726b8c`. It then changed `app.js`, `server.mjs`
and `check.mjs`, published fingerprint
`sha256:3fa1f27a53e50cf1ae90c1d3b1f0b2b7d3be0586b5d039da2bd10d9b060de4b0`,
and repeated the same guest interaction inside a second browser observation.

The pre-fix browser observation recorded one client-side `POST /api/cards` and
its 200 response. The visible client state showed one newly returned card; the
evaluator proxy's repeated downstream delivery is below the browser transport
and therefore is not misreported as two CDP requests. Source inspection showed
that every delivery generated and persisted a new UUID. The agent diagnosed
the missing replay identity and unsynchronized mutations, then implemented
keyed replay plus a bounded compatibility path for keyless clients. The
evaluator-owned fresh-state scorer independently exercised repeated delivery.

After publication, a second overlapped observation recorded another POST/200
pair. The delayed authoritative framebuffer contained exactly one newly added
card and the backlog count increased from 2 to 3. The `N` shortcut's default
key action also inserted a leading `n` in the title; that visible input artifact
is retained rather than edited out and does not affect the one-create result.
Two compact replay artifacts were produced after the interaction.

## Independent acceptance

The machine-readable final audit accepted publish ordinal 1 and passed every
check: independent evaluator, canonical timeline, browser/network correlation,
pre-edit diagnosis inputs, temporal revisit, historical/replay inspection,
publication, repeated graphical input and corrected browser evidence.

- canonical timeline events: 1,696
- browser network requests/responses: 2/2 (one pair before and one after)
- recorded guest interactions: 169
- agent exit/timed out: 0/false
- MCP tool failures: 0
- evaluator functional defects/regressions: 0/0
- evaluator duplicate count: 1
- evaluator project-check exit code: 0
- final GCE state: `TERMINATED`

The authoritative framebuffer PNG hashes are
`7a1530b506a888c5be6de307b15cd2f0535753f7f4656f487d4bdb1f214f54dd`
(pre-fix interaction) and
`dc358a794ac05209024f9a1465d93aa9d56df1fd6aa17030217e48ab03dc8e5e`
(post-fix interaction). Their raw-frame hashes reported by AVM are
`bbfd86fdf0977c1ab3edf329778732d7dcd806235460bef750214cf41aa5a91c`
and `73fbb2aecc39596f66bcd7986575adfeec19a654944bb6b3996160e725cd988c`.

## Evidence integrity

| Artifact | SHA-256 |
| --- | --- |
| `agent-metrics.json` | `85e0d8411b14af6dd7c9de82d84984aa9ebf845e39c0dcc1c5a3ab7c3f09586d` |
| `codex-events.jsonl` | `65b355a7ff24221db1b53283ea1f95bdf667020907b46e0354e2b1f92de33994` |
| `codex-final.txt` | `006c460f041faf6bd2e76300e81f19594c812509ac18a4ca09cec6f72af36208` |
| `remote-history.json` | `942ebde2ef2577cd7b3c02a762bf4427485fd0f0acf9c49bcf6cc5e033eda49e` |
| `evaluator-score.json` | `33f30f1ffb5df4bf2b3779e2ed47d0c09bc965ddffc8ab1b908291483830b64c` |
| `final-audit.json` | `f4ad5abe91824bad11815be3b9bce7832df8e037051c948c2e27723e4aaa83ee` |

The no-model overlap preflight that gated this run is
`/Users/artpar/workspace/code/avm-evidence/final-experience-loop/preflight-overlap-4`.
It recorded 673 browser events, 2 network events, 108 guest interactions and
trace `sha256:0be008d91fed954b9280d7ebd23809d1c90f3184ea979455d1bc265ea69b6a43`.
Its result/history hashes are
`fe93456560f92ba585075a77beb65aa626840b98022ab79124c3bf29bbfae002`
and `ea736109ecd6396fe95cb9e37a9af8e500543d47ae37e0d69bdb0626596e7765`.

This gate proves the workstation can support the required loop. It does not
change the controlled experiment's outcome: on the tested task/model, rich
perception produced no functional-defect main effect and cost substantially
more time, tools and tokens.
