#!/usr/bin/env bash
set -euo pipefail

ssh_key=${1:?usage: wait-for-provisioning.sh SSH_KEY QEMU_PID SERIAL_LOG BUILD_LOG}
build_pid=${2:?usage: wait-for-provisioning.sh SSH_KEY QEMU_PID SERIAL_LOG BUILD_LOG}
guest_serial_log=${3:?usage: wait-for-provisioning.sh SSH_KEY QEMU_PID SERIAL_LOG BUILD_LOG}
build_log=${4:?usage: wait-for-provisioning.sh SSH_KEY QEMU_PID SERIAL_LOG BUILD_LOG}
provision_timeout=${AVM_IMAGE_PROVISION_TIMEOUT_SECONDS:-1800}
poll_seconds=${AVM_IMAGE_PROVISION_POLL_SECONDS:-2}

case "$provision_timeout" in
  ''|*[!0-9]*|0) echo "AVM_IMAGE_PROVISION_TIMEOUT_SECONDS must be a positive integer" >&2; exit 2 ;;
esac
case "$poll_seconds" in
  ''|*[!0-9]*|0) echo "AVM_IMAGE_PROVISION_POLL_SECONDS must be a positive integer" >&2; exit 2 ;;
esac

guest_ssh() {
  ssh -o BatchMode=yes -o ConnectTimeout=2 -o StrictHostKeyChecking=no \
    -o UserKnownHostsFile=/dev/null -i "$ssh_key" -p 22222 avm@127.0.0.1 "$@"
}

print_diagnostics() {
  echo "cloud-init status:" >&2
  guest_ssh 'cloud-init status --long' >&2 || true
  echo "last 80 guest serial lines ($guest_serial_log):" >&2
  tail -n 80 "$guest_serial_log" >&2 2>/dev/null || true
  echo "last 80 QEMU log lines ($build_log):" >&2
  tail -n 80 "$build_log" >&2 2>/dev/null || true
}

provision_started_seconds=$SECONDS
while (( SECONDS - provision_started_seconds < provision_timeout )); do
  if ! kill -0 "$build_pid" 2>/dev/null; then
    echo "guest exited before provisioning completed" >&2
    print_diagnostics
    exit 1
  fi

  if guest_ssh 'test -f /var/lib/avm-image-ready' 2>/dev/null; then
    set +e
    cloud_status=$(guest_ssh 'cloud-init status --wait --long' 2>&1)
    cloud_status_exit=$?
    set -e
    printf '%s\n' "$cloud_status"
    if (( cloud_status_exit != 0 )); then
      echo "cloud-init completed with semantic provisioning errors (exit $cloud_status_exit)" >&2
      print_diagnostics
      exit 1
    fi
    exit 0
  fi
  sleep "$poll_seconds"
done

echo "guest provisioning exceeded ${provision_timeout}s without a readiness marker" >&2
print_diagnostics
exit 1
