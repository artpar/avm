#!/usr/bin/env bash
set -euo pipefail

if [[ $(uname -s) != Linux ]]; then
  echo "SKIP: graphical WebGL2 evidence requires Linux" >&2
  exit 77
fi

run_config=${1:?usage: linux-webgl-smoke.sh RUN_CONFIG [HOST_PORT]}
host_port=${2:-31880}
avm_bin=${AVM_BIN:-avm}
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
install_root=$(cd -- "$script_dir/.." && pwd)
fixture_dir=${AVM_WEBGL_FIXTURE:-}
if [[ -z "$fixture_dir" ]]; then
  for candidate in \
      "$install_root/share/avm/fixtures/webgl2" \
      "$install_root/fixtures/webgl2"; do
    if [[ -f "$candidate/index.html" ]]; then
      fixture_dir=$candidate
      break
    fi
  done
fi
[[ -f "$fixture_dir/index.html" && -f "$fixture_dir/app.js" ]] || {
  echo "AVM WebGL2 fixture is not installed" >&2
  exit 2
}
png_check=${AVM_PNG_REGION_CHECK:-}
if [[ -z "$png_check" ]]; then
  for candidate in \
      "$script_dir/png-region-check.py" \
      "$install_root/libexec/avm/png-region-check.py"; do
    if [[ -f "$candidate" ]]; then
      png_check=$candidate
      break
    fi
  done
fi
[[ -f "$png_check" ]] || { echo "AVM PNG checker is not installed" >&2; exit 2; }

if [[ "$avm_bin" == */* ]]; then
  [[ -x "$avm_bin" ]] || { echo "AVM executable is not runnable: $avm_bin" >&2; exit 2; }
else
  command -v "$avm_bin" >/dev/null || { echo "AVM executable is not on PATH: $avm_bin" >&2; exit 2; }
fi

run_dir=$(dirname -- "$run_config")
base_image=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["baseImage"])' "$run_config")
ssh_key=${AVM_GUEST_SSH_KEY:-"$(dirname -- "$base_image")/avm_ed25519"}
url="http://10.0.2.2:${host_port}/index.html"
server_log=$(mktemp)
server_pid=''
cleanup() {
  if [[ -n "$server_pid" ]] && kill -0 "$server_pid" 2>/dev/null; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -f -- "$server_log"
}
trap cleanup EXIT INT TERM

python3 -m http.server "$host_port" --bind 0.0.0.0 --directory "$fixture_dir" \
  >"$server_log" 2>&1 &
server_pid=$!
for _attempt in $(seq 1 50); do
  curl -fsS "http://127.0.0.1:${host_port}/index.html" >/dev/null 2>&1 && break
  sleep 0.1
done
curl -fsS "http://127.0.0.1:${host_port}/index.html" >/dev/null || {
  cat "$server_log" >&2
  exit 1
}

ssh_guest() {
  ssh -o BatchMode=yes -o ConnectTimeout=2 -o StrictHostKeyChecking=no \
    -o UserKnownHostsFile=/dev/null -i "$ssh_key" -p 2222 avm@127.0.0.1 "$@"
}

wait_for_desktop() {
  local ready=false
  for _attempt in $(seq 1 180); do
    if ssh_guest \
        'systemctl is-active --quiet weston.service && pgrep -x chrome >/dev/null' \
        2>/dev/null; then
      ready=true
      break
    fi
    sleep 1
  done
  [[ "$ready" == true ]] || { echo "guest desktop did not become ready" >&2; exit 1; }
}

wait_for_title() {
  local expected_title=$1
  local ready=false
  for _attempt in $(seq 1 60); do
    if ssh_guest 'curl -fsS http://127.0.0.1:9222/json/list' 2>/dev/null \
        | python3 -c 'import json,sys; expected=sys.argv[1]; raise SystemExit(0 if any(page.get("title") == expected for page in json.load(sys.stdin)) else 1)' "$expected_title"; then
      ready=true
      break
    fi
    sleep 1
  done
  [[ "$ready" == true ]] || { echo "guest page did not report $expected_title" >&2; exit 1; }
}

running=$("$avm_bin" status --run "$run_config" \
  | python3 -c 'import json,sys; print("true" if json.load(sys.stdin)["running"] else "false")')
if [[ "$running" != true ]]; then
  "$avm_bin" start --run "$run_config"
fi
wait_for_desktop
"$avm_bin" smoke --run "$run_config" --url "$url" --screenshot "$run_dir/webgl2-before.png"
wait_for_title AVM_WEBGL2_OK
"$avm_bin" capture --run "$run_config" --output "$run_dir/webgl2-before.png"
python3 "$png_check" "$run_dir/webgl2-before.png" 32 191 64

# Weston consumes the first click when it focuses the Chromium window.
"$avm_bin" act-click --run "$run_config" --x 300 --y 300 --wait-after-ms 250 >/dev/null
"$avm_bin" act-click --run "$run_config" --x 300 --y 300 --wait-after-ms 500
wait_for_title AVM_WEBGL2_UPDATED
"$avm_bin" capture --run "$run_config" --output "$run_dir/webgl2-after.png"
python3 "$png_check" "$run_dir/webgl2-after.png" 230 51 204

test -s "$run_dir/events.jsonl"
echo "PASS: WebGL2 context, browser state, framebuffer colors, and input-driven repaint recorded"
