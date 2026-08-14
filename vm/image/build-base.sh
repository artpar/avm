#!/usr/bin/env bash
set -euo pipefail

if [[ $(uname -s) != Linux ]]; then
  echo "guest image construction requires a Linux host" >&2
  exit 2
fi

for command_name in curl qemu-img qemu-system-x86_64 cloud-localds ssh ssh-keygen sha256sum; do
  command -v "$command_name" >/dev/null || { echo "missing prerequisite: $command_name" >&2; exit 2; }
done

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
source "$script_dir/ubuntu-noble-amd64.lock"
output_dir=${1:?usage: build-base.sh OUTPUT_DIRECTORY}
mkdir -p "$output_dir"
output_dir=$(realpath "$output_dir")
source_image="$output_dir/upstream.qcow2"
base_image="$output_dir/avm-base.qcow2"
seed_image="$output_dir/seed.img"
ssh_key="$output_dir/avm_ed25519"
build_log="$output_dir/build-qemu.log"
guest_serial_log="$output_dir/guest-serial.log"
build_pid=''

cleanup() {
  if [[ -n "$build_pid" ]] && kill -0 "$build_pid" 2>/dev/null; then
    kill "$build_pid" 2>/dev/null || true
    wait "$build_pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

if [[ ! -f "$source_image" ]]; then
  curl --fail --location --proto '=https' --tlsv1.2 "$URL" --output "$source_image"
fi
printf '%s  %s\n' "$SHA256" "$source_image" | sha256sum --check --status || {
  echo "upstream image hash mismatch; refusing mutable release content" >&2
  exit 1
}

cp --reflink=auto "$source_image" "$base_image"
qemu-img resize "$base_image" 20G
if [[ ! -f "$ssh_key" ]]; then
  ssh-keygen -q -t ed25519 -N '' -C avm-image-builder -f "$ssh_key"
fi

public_key=$(<"$ssh_key.pub")
accessibility_sensor=$(base64 -w0 "$script_dir/../guest/avm-accessibility-sensor.py")
sed -e "s|@@SSH_PUBLIC_KEY@@|$public_key|" \
  -e "s|@@ACCESSIBILITY_SENSOR_B64@@|$accessibility_sensor|" \
  "$script_dir/user-data.yaml" >"$output_dir/user-data"
cp "$script_dir/meta-data.yaml" "$output_dir/meta-data"
cloud-localds "$seed_image" "$output_dir/user-data" "$output_dir/meta-data"

qemu-system-x86_64 \
  -machine q35,accel=kvm -cpu host -m 4096 -smp 4 \
  -drive "if=virtio,format=qcow2,file=$base_image" \
  -drive "if=virtio,format=raw,readonly=on,file=$seed_image" \
  -netdev user,id=net0,hostfwd=tcp:127.0.0.1:22222-:22 \
  -device virtio-net-pci,netdev=net0 -display none \
  -serial "file:$guest_serial_log" -monitor none \
  -daemonize -pidfile "$output_dir/build-qemu.pid" -D "$build_log"
build_pid=$(<"$output_dir/build-qemu.pid")

ready=false
for _attempt in $(seq 1 180); do
  if ssh -o BatchMode=yes -o ConnectTimeout=2 -o StrictHostKeyChecking=no \
      -o UserKnownHostsFile=/dev/null -i "$ssh_key" -p 22222 avm@127.0.0.1 \
      'test -f /var/lib/avm-image-ready' 2>/dev/null; then
    ready=true
    break
  fi
  sleep 2
done
if [[ "$ready" != true ]]; then
  echo "guest provisioning did not become ready; see $build_log" >&2
  exit 1
fi

ssh -o BatchMode=yes -o ConnectTimeout=5 -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null -i "$ssh_key" -p 22222 avm@127.0.0.1 \
  'cloud-init status --wait'

ssh -o BatchMode=yes -o ConnectTimeout=5 -o StrictHostKeyChecking=no \
  -o UserKnownHostsFile=/dev/null -i "$ssh_key" -p 22222 avm@127.0.0.1 \
  'sudo systemctl poweroff'

for _attempt in $(seq 1 120); do
  kill -0 "$build_pid" 2>/dev/null || break
  sleep 1
done
if kill -0 "$build_pid" 2>/dev/null; then
  echo "guest did not power off after provisioning" >&2
  exit 1
fi
build_pid=''

rm -f "$seed_image" "$output_dir/user-data" "$output_dir/meta-data" "$output_dir/build-qemu.pid"
qemu-img convert -p -O qcow2 -o compat=1.1,lazy_refcounts=on "$base_image" "$base_image.compacted"
mv "$base_image.compacted" "$base_image"
sha256sum "$base_image" >"$base_image.sha256"
echo "$base_image"
