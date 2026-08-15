#!/usr/bin/env bash
set -euo pipefail

test_root=$(mktemp -d)
trap 'rm -rf "$test_root"' EXIT
fake_bin="$test_root/bin"
mkdir -p "$fake_bin"
printf '%s\n' serial-progress >"$test_root/serial.log"
printf '%s\n' qemu-detail >"$test_root/qemu.log"
: >"$test_root/key"

cat >"$fake_bin/ssh" <<'EOF'
#!/bin/sh
for argument in "$@"; do command=$argument; done
case "$command" in
  'test -f /var/lib/avm-image-ready') exit "${FAKE_MARKER_EXIT:-0}" ;;
  'cloud-init status --wait --long')
    printf '%s\n' "${FAKE_CLOUD_WAIT_OUTPUT:-status: done}"
    exit "${FAKE_CLOUD_WAIT_EXIT:-0}"
    ;;
  'cloud-init status --long') printf '%s\n' "${FAKE_CLOUD_DIAGNOSTIC:-status: running}" ;;
  *) exit 2 ;;
esac
EOF
chmod +x "$fake_bin/ssh"

run_wait() {
  PATH="$fake_bin:$PATH" \
  AVM_IMAGE_PROVISION_TIMEOUT_SECONDS=${AVM_IMAGE_PROVISION_TIMEOUT_SECONDS:-5} \
  AVM_IMAGE_PROVISION_POLL_SECONDS=1 \
    vm/image/wait-for-provisioning.sh \
      "$test_root/key" "$$" "$test_root/serial.log" "$test_root/qemu.log"
}

success_output=$(run_wait)
grep -q 'status: done' <<<"$success_output"

set +e
semantic_output=$(FAKE_CLOUD_WAIT_EXIT=2 \
  FAKE_CLOUD_WAIT_OUTPUT='status: degraded done' run_wait 2>&1)
semantic_exit=$?
set -e
[[ $semantic_exit -eq 1 ]]
grep -q 'semantic provisioning errors (exit 2)' <<<"$semantic_output"
grep -q 'status: degraded done' <<<"$semantic_output"

set +e
timeout_output=$(FAKE_MARKER_EXIT=1 \
  AVM_IMAGE_PROVISION_TIMEOUT_SECONDS=1 run_wait 2>&1)
timeout_exit=$?
set -e
[[ $timeout_exit -eq 1 ]]
grep -q 'exceeded 1s without a readiness marker' <<<"$timeout_output"
grep -q 'status: running' <<<"$timeout_output"
grep -q 'serial-progress' <<<"$timeout_output"
grep -q 'qemu-detail' <<<"$timeout_output"

echo "image provisioning wait contract passed"
