#!/usr/bin/env bash
set -euo pipefail

if [[ $(uname -s) != Linux ]]; then
  echo "SKIP: physical QEMU/KVM evidence requires Linux" >&2
  exit 77
fi

run_config=${1:?usage: linux-smoke.sh RUN_CONFIG [URL]}
url=${2:-http://10.0.2.2:3000}
avm_bin=${AVM_BIN:-avm}
if [[ "$avm_bin" == */* ]]; then
  [[ -x "$avm_bin" ]] || { echo "AVM executable is not runnable: $avm_bin" >&2; exit 2; }
else
  command -v "$avm_bin" >/dev/null || { echo "AVM executable is not on PATH: $avm_bin" >&2; exit 2; }
fi
run_dir=$(dirname -- "$run_config")
base_image=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["baseImage"])' "$run_config")
ssh_key=${AVM_GUEST_SSH_KEY:-"$(dirname -- "$base_image")/avm_ed25519"}

wait_for_desktop() {
  local ready=false
  for _attempt in $(seq 1 180); do
    if ssh -o BatchMode=yes -o ConnectTimeout=2 -o StrictHostKeyChecking=no \
        -o UserKnownHostsFile=/dev/null -i "$ssh_key" -p 2222 avm@127.0.0.1 \
        'test -f /workspace/index.html && systemctl is-active --quiet weston.service && pgrep -x chrome >/dev/null' \
        2>/dev/null; then
      ready=true
      break
    fi
    sleep 1
  done
  [[ "$ready" == true ]] || { echo "guest desktop did not become ready" >&2; exit 1; }
}

wait_for_page() {
  local expected_url=$1
  local ready=false
  for _attempt in $(seq 1 60); do
    if ssh -o BatchMode=yes -o ConnectTimeout=2 -o StrictHostKeyChecking=no \
        -o UserKnownHostsFile=/dev/null -i "$ssh_key" -p 2222 avm@127.0.0.1 \
        'curl -fsS http://127.0.0.1:9222/json/list' 2>/dev/null \
        | python3 -c 'import json,sys; expected=sys.argv[1].rstrip("/"); raise SystemExit(0 if any(page.get("url", "").rstrip("/") == expected for page in json.load(sys.stdin)) else 1)' "$expected_url"; then
      ready=true
      break
    fi
    sleep 1
  done
  [[ "$ready" == true ]] || { echo "guest page did not navigate to $expected_url" >&2; exit 1; }
}

running=$("$avm_bin" status --run "$run_config" \
  | python3 -c 'import json,sys; print("true" if json.load(sys.stdin)["running"] else "false")')
if [[ "$running" != true ]]; then
  "$avm_bin" start --run "$run_config"
fi
wait_for_desktop
"$avm_bin" smoke --run "$run_config" --url "$url" --screenshot "$run_dir/smoke.png"
wait_for_page "$url"
"$avm_bin" capture --run "$run_config" --output "$run_dir/smoke.png"
"$avm_bin" drag-proof --run "$run_config" --from-x 400 --from-y 350 --to-x 800 --to-y 350
"$avm_bin" reset --run "$run_config"
wait_for_desktop
"$avm_bin" capture --run "$run_config" --output "$run_dir/reset.png"

test -s "$run_dir/smoke.png"
test -s "$run_dir/reset.png"
test -s "$run_dir/events.jsonl"
echo "PASS: real scanout, injected navigation input, post-input display update, drag-time updates, and QMP snapshot reset recorded"
