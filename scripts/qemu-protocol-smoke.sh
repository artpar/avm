#!/usr/bin/env bash
set -euo pipefail

if [[ $(uname -s) != Linux ]]; then
  echo "SKIP: real QEMU protocol smoke requires Linux" >&2
  exit 77
fi

for command_name in cargo dbus-daemon nasm python3 qemu-img qemu-system-x86_64 sha256sum; do
  command -v "$command_name" >/dev/null || {
    echo "missing prerequisite: $command_name" >&2
    exit 2
  }
done

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
project_dir=$(cd -- "$script_dir/.." && pwd)
accelerator=${AVM_QEMU_ACCELERATOR:-tcg}
if [[ "$accelerator" != tcg && "$accelerator" != kvm ]]; then
  echo "AVM_QEMU_ACCELERATOR must be tcg or kvm" >&2
  exit 2
fi
evidence_dir=''
if [[ $# -gt 0 ]]; then
  evidence_dir=$1
  if [[ -d "$evidence_dir" ]] && [[ -n $(find "$evidence_dir" -mindepth 1 -maxdepth 1 -print -quit) ]]; then
    echo "evidence output directory must be empty: $evidence_dir" >&2
    exit 2
  fi
  mkdir -p "$evidence_dir"
  evidence_dir=$(realpath "$evidence_dir")
fi
probe_dir=$(mktemp -d)
dbus_pid=''
qemu_pid=''
display_socket=''

cleanup() {
  if [[ -n "$qemu_pid" ]]; then
    kill "$qemu_pid" 2>/dev/null || true
    wait "$qemu_pid" 2>/dev/null || true
  fi
  if [[ -n "$dbus_pid" ]]; then
    kill "$dbus_pid" 2>/dev/null || true
    wait "$dbus_pid" 2>/dev/null || true
  fi
  if [[ -n "$display_socket" ]]; then
    rm -f -- "$display_socket" 2>/dev/null || true
  fi
  if [[ -f "$probe_dir/result.json" ]]; then
    (
      cd "$probe_dir"
      : >checksums.sha256
      for artifact in *; do
        if [[ -f "$artifact" && "$artifact" != checksums.sha256 ]]; then
          sha256sum "$artifact" >>checksums.sha256
        fi
      done
    )
  fi
  if [[ -n "$evidence_dir" ]]; then
    for evidence_file in "$probe_dir"/*; do
      if [[ -f "$evidence_file" ]]; then
        cp -- "$evidence_file" "$evidence_dir/"
      fi
    done
  fi
  rm -rf -- "$probe_dir"
}
trap cleanup EXIT INT TERM

mkdir -p "$probe_dir/candidate"
nasm -f bin "$script_dir/fixtures/input-vga.asm" -o "$probe_dir/input-vga.img"
if [[ $(wc -c <"$probe_dir/input-vga.img") -ne 512 ]]; then
  echo "boot fixture must be exactly 512 bytes" >&2
  exit 1
fi
qemu-img convert -f raw -O qcow2 "$probe_dir/input-vga.img" "$probe_dir/overlay.qcow2"

display_socket="$probe_dir/display.sock"
dbus-daemon --session --nofork \
  --address="unix:path=$display_socket" \
  --print-address=1 >"$probe_dir/dbus.log" 2>&1 &
dbus_pid=$!

for _attempt in $(seq 1 100); do
  [[ -S "$display_socket" ]] && break
  kill -0 "$dbus_pid" 2>/dev/null || {
    echo "private D-Bus daemon exited" >&2
    exit 1
  }
  sleep 0.02
done
[[ -S "$display_socket" ]] || { echo "private D-Bus socket did not appear" >&2; exit 1; }

qemu_system_args=(
  -name avm-protocol-probe
  -machine "pc,accel=$accelerator"
  -m 64
  -blockdev "driver=file,node-name=os-file,filename=$probe_dir/overlay.qcow2"
  -blockdev driver=qcow2,node-name=os,file=os-file
  -device floppy,drive=os
  -boot a
  -qmp "unix:$probe_dir/qmp.sock,server=on,wait=off"
  -display "dbus,addr=unix:path=$display_socket,gl=off"
  -debugcon "file:$probe_dir/guest-debug.log"
  -global isa-debugcon.iobase=0xe9
  -monitor none
  -serial none
  -no-reboot
)
if [[ "$accelerator" == kvm ]]; then
  qemu_system_args+=( -cpu host )
fi
printf '%q ' qemu-system-x86_64 "${qemu_system_args[@]}" >"$probe_dir/qemu-argv.txt"
printf '\n' >>"$probe_dir/qemu-argv.txt"
{
  uname -a
  qemu-system-x86_64 --version
  cargo --version
  rustc --version
} >"$probe_dir/environment.txt"
qemu-system-x86_64 "${qemu_system_args[@]}" >"$probe_dir/qemu.log" 2>&1 &
qemu_pid=$!
printf '%s\n' "$qemu_pid" >"$probe_dir/qemu.pid"

guest_ready=false
for _attempt in $(seq 1 2400); do
  if [[ -f "$probe_dir/guest-debug.log" ]] && grep -q READY "$probe_dir/guest-debug.log"; then
    guest_ready=true
    break
  fi
  kill -0 "$qemu_pid" 2>/dev/null || {
    echo "QEMU exited before the fixture became ready" >&2
    sed -n '1,200p' "$probe_dir/qemu.log" >&2
    exit 1
  }
  sleep 0.05
done
if [[ "$guest_ready" != true ]]; then
  echo "boot fixture did not become ready" >&2
  sed -n '1,200p' "$probe_dir/qemu.log" >&2
  exit 1
fi
# READY is synchronous guest execution; QEMU's display refresh is asynchronous.
# Let the already-painted VGA surface become the current host scanout before
# attaching the listener, so the later update is attributable to injected input.
sleep 1

run_id=00000000-0000-0000-0000-000000000001
run_config="$probe_dir/run.json"
printf '{\n  "id": "%s",\n  "baseImage": "%s",\n  "candidateWorkspace": "%s",\n  "stateDir": "%s",\n  "memoryMib": 64,\n  "cpus": 1,\n  "width": 320,\n  "height": 200\n}\n' \
  "$run_id" "$probe_dir/input-vga.img" "$probe_dir/candidate" "$probe_dir" >"$run_config"

(cd "$project_dir" && cargo run --quiet -- checkpoint --run "$run_config")

smoke_output=$(cd "$project_dir" && cargo run --quiet -- smoke \
  --run "$run_config" \
  --url a \
  --screenshot "$probe_dir/after-input.png") || {
    echo "QEMU log:" >&2
    sed -n '1,200p' "$probe_dir/qemu.log" >&2
    echo "D-Bus log:" >&2
    sed -n '1,200p' "$probe_dir/dbus.log" >&2
    exit 1
  }

test -s "$probe_dir/after-input.png"
test -s "$probe_dir/events.jsonl"
grep -q '"kind":"display.scanout"' "$probe_dir/events.jsonl"
grep -q '"kind":"key.down"' "$probe_dir/events.jsonl"
guest_key_count=$(tr -cd K <"$probe_dir/guest-debug.log" | wc -c)
if [[ "$guest_key_count" -lt 3 ]]; then
  echo "guest acknowledged only $guest_key_count injected keys" >&2
  exit 1
fi

read -r pre_input_hash post_input_hash < <(python3 - "$probe_dir/events.jsonl" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    events = [json.loads(line) for line in stream if line.strip()]
first_input_ns = min(
    event["hostMonotonicNs"] for event in events if event["source"] == "input"
)
display = [
    event for event in events
    if event["kind"] in ("display.scanout", "display.update")
]
pre = max(
    (event for event in display if event["hostMonotonicNs"] < first_input_ns),
    key=lambda event: event["hostMonotonicNs"],
)
pre_hash = pre["payload"]["frameSha256"]
post = min(
    (
        event for event in display
        if event["hostMonotonicNs"] > first_input_ns
        and event["payload"]["frameSha256"] != pre_hash
    ),
    key=lambda event: event["hostMonotonicNs"],
)
print(pre_hash, post["payload"]["frameSha256"])
PY
)
if [[ -z "$pre_input_hash" || -z "$post_input_hash" || "$pre_input_hash" == "$post_input_hash" ]]; then
  echo "no changed framebuffer was recorded after input" >&2
  exit 1
fi

(cd "$project_dir" && cargo run --quiet -- restore-checkpoint --run "$run_config")
reset_output=$(cd "$project_dir" && cargo run --quiet -- capture \
  --run "$run_config" \
  --output "$probe_dir/after-reset.png")
reset_hash=$(printf '%s\n' "$reset_output" | \
  sed -n 's/.*"frameSha256":"\([0-9a-f]*\)".*/\1/p')
if [[ -z "$reset_hash" || "$reset_hash" != "$pre_input_hash" ]]; then
  echo "snapshot reset framebuffer $reset_hash did not restore $pre_input_hash" >&2
  exit 1
fi

printf '%s\n' "$smoke_output"
printf '%s\n' "$smoke_output" >"$probe_dir/smoke-result.json"
printf '{"accepted":true,"accelerator":"%s","guestKeyAcknowledgments":%s,"preInputFrameSha256":"%s","postInputFrameSha256":"%s","resetFrameSha256":"%s","snapshotRestored":true,"timeline":"events.jsonl","screenshot":"after-input.png","resetScreenshot":"after-reset.png"}\n' \
  "$accelerator" "$guest_key_count" "$pre_input_hash" "$post_input_hash" "$reset_hash" >"$probe_dir/result.json"
echo "PASS: real QEMU $accelerator scanout -> recorded input -> guest repaint -> QMP snapshot restore"
