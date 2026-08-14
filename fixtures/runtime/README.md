# Runtime telemetry fixture

`telemetry.jsonl` exercises the bounded runtime importer with one span, one trace-correlated application log, one process exit, and one profiling sample. Import it into an existing run with:

```sh
target/release/avm runtime-import \
  --run /var/lib/avm/runs/RUN_ID/run.json \
  --input fixtures/runtime/telemetry.jsonl
```

The importer validates the complete batch before storing its raw bytes or recording events. Trace IDs and parent span IDs are instrumentation declarations. Events that merely occur nearby remain temporal context and are not reported as causal.
