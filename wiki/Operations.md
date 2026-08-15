# Operations

Set a shell variable once for examples:

```sh
RUN_CONFIG=/var/lib/avm/runs/RUN_ID/run.json
```

## Lifecycle

```sh
avm start --run "$RUN_CONFIG"
avm status --run "$RUN_CONFIG"
avm checkpoint --run "$RUN_CONFIG" --name before-change
avm restore-checkpoint --run "$RUN_CONFIG" --name before-change
avm reset --run "$RUN_CONFIG"
avm stop --run "$RUN_CONFIG"
```

Use `destroy-run` only when its externally stored run state and overlay are no
longer needed. Keep base images read-only.

## Guest actions

```sh
avm act-pointer --run "$RUN_CONFIG" --x 640 --y 360
avm act-click --run "$RUN_CONFIG" --x 640 --y 360 --wait-after-ms 500
avm act-type --run "$RUN_CONFIG" --text 'example'
avm act-key --run "$RUN_CONFIG" --keycode 28 --mode press
avm act-drag --run "$RUN_CONFIG" \
  --from-x 400 --from-y 300 --to-x 700 --to-y 500 \
  --steps 12 --duration-ms 500
avm act-scroll --run "$RUN_CONFIG" --delta-y 3
```

Every high-level action returns an action ID and start/completion timestamps.
Low-level input events retain the same ID.

## Present and historical state

```sh
avm capture --run "$RUN_CONFIG" --output /tmp/current.png
avm observe --run "$RUN_CONFIG" --recent-limit 20
avm history --run "$RUN_CONFIG" --last-duration-ms 10000
avm frame --run "$RUN_CONFIG" --at-ns MONOTONIC_NS --output /tmp/past.png
avm replay --run "$RUN_CONFIG" --last-duration-ms 10000
```

`frame` and `replay` operate from durable evidence even when QEMU is stopped.

## Browser and accessibility

With the guest CDP port tunneled to host loopback:

```sh
avm browser-observe --run "$RUN_CONFIG" \
  --endpoint http://127.0.0.1:9223 \
  --script /path/to/supervisor/browser/observer.mjs \
  --duration-ms 10000

avm accessibility-observe --run "$RUN_CONFIG" --duration-ms 5000
```

For network/action correlation, start browser observation before real AVM input
and keep it active until after the guest response.

## Operational rules

- Create a new run after the outer host reboots.
- Keep supervisor state and channel configuration outside candidates.
- Stop nested QEMU and the outer cloud instance when idle.
- Never publish VM disks, SSH private keys, raw run stores, or private evidence.
- Treat short performance measurements as scale estimates, not causal proof.
