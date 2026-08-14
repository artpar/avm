# Evaluator-owned boundary

This directory must remain outside candidate workspaces. `prepare-candidate.mjs` copies only `../target-app`; the reverse proxy, profiles, and hidden scoring stay supervisor-owned.

Start the candidate server on port 3000, then expose one evaluator profile on port 3001:

```sh
FAULT_PROFILE=$PWD/profiles/temporal.json \
TARGET_ORIGIN=http://127.0.0.1:3000 EVALUATOR_PORT=3001 \
node fault-proxy.mjs
```

The candidate task names a user-facing symptom, not the profile or complete fault list. Final scoring runs against a fresh candidate copy and starting state. Run `npm run check` here to prove that a single client action can be externally transformed into the double-action defect without placing evaluator code in the candidate.
