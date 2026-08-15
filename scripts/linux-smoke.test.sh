#!/usr/bin/env bash
set -euo pipefail

test_root=$(mktemp -d)
trap 'rm -rf "$test_root"' EXIT
fake_bin="$test_root/bin"
run_dir="$test_root/run"
image_dir="$test_root/image"
mkdir -p "$fake_bin" "$run_dir" "$image_dir"

cat >"$fake_bin/uname" <<'EOF'
#!/bin/sh
printf '%s\n' Linux
EOF

cat >"$fake_bin/ssh" <<'EOF'
#!/bin/sh
case "$*" in
  *json/list*) printf '%s\n' '[{"url":"http://10.0.2.2:3000"}]' ;;
esac
EOF

cat >"$fake_bin/avm" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$FAKE_AVM_LOG"
previous=''
for argument in "$@"; do
  if [ "$previous" = --run ]; then
    printf '%s\n' '{"event":"fixture"}' >>"$(dirname -- "$argument")/events.jsonl"
  fi
  previous=$argument
done
case "$1" in
  status)
    printf '{"running":%s}\n' "$FAKE_AVM_RUNNING"
    ;;
  smoke|capture)
    previous=''
    for argument in "$@"; do
      case "$previous" in
        --screenshot|--output) printf '%s' png >"$argument" ;;
      esac
      previous=$argument
    done
    ;;
esac
EOF

chmod +x "$fake_bin/uname" "$fake_bin/ssh" "$fake_bin/avm"
printf '{"baseImage":"%s/avm-base.qcow2"}\n' "$image_dir" >"$run_dir/run.json"
: >"$run_dir/events.jsonl"

run_smoke() {
  FAKE_AVM_LOG="$test_root/avm.log" \
  FAKE_AVM_RUNNING=$1 \
  AVM_BIN="$fake_bin/avm" \
  PATH="$fake_bin:$PATH" \
    bash scripts/linux-smoke.sh "$run_dir/run.json"
}

run_smoke true
if grep -q '^start ' "$test_root/avm.log"; then
  echo "smoke helper restarted an already-running VM" >&2
  exit 1
fi
grep -q '^status ' "$test_root/avm.log"
grep -q '^smoke ' "$test_root/avm.log"
grep -q '^capture ' "$test_root/avm.log"
grep -q '^drag-proof ' "$test_root/avm.log"
grep -q '^reset ' "$test_root/avm.log"

: >"$test_root/avm.log"
run_smoke false
grep -q '^start ' "$test_root/avm.log"

if grep -q 'cargo' "$test_root/avm.log"; then
  echo "smoke helper invoked Cargo" >&2
  exit 1
fi

echo "linux smoke helper contract passed"
