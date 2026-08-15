# Troubleshooting

## `/dev/kvm` is missing or inaccessible

Confirm CPU virtualization and nested virtualization are enabled, load the KVM
kernel modules, and check the current user's group access. The TCG protocol
smoke test can exercise the QEMU bridge without KVM, but it does not replace the
full Linux acceptance test.

## QEMU starts but no framebuffer appears

Check the private D-Bus address, display listener socket path, and QEMU logs.
Unix socket paths have a platform length limit, so avoid deeply nested run
roots. Verify that the pinned guest finished cloud-init and launched Weston.

## Input is recorded but the application does not react

Capture before and after frames and inspect `history --source input`. Confirm
that the target window has focus and that the Linux input keycode is correct.
Use `scripts/linux-smoke.sh` to distinguish a general input/display problem from
an application problem.

## Browser observation cannot connect

Confirm Chromium is running with CDP on guest loopback port 9222, the scoped SSH
tunnel maps that port to the configured `browserEndpoint`, and
`/json/version` responds through the tunnel. CDP should never bind publicly.

## MCP calls fail before reaching AVM

Run `node supervisor/mcp/check.mjs`, validate the JSON config, and confirm every
required path is absolute. Then run the equivalent `gcloud compute ssh` command
with the same explicit project, zone, and instance. Codex authentication on the
VM is neither needed nor useful.

## A run stopped working after host reboot

Runs are boot-bound. Create a new run after rebooting the outer GCE/KVM host;
do not append new events to the old run.

## Release checksum fails

Download both the archive and its matching `.sha256` file from the same GitHub
Release. Do not install the binary. If the files are unchanged and still fail,
open a bug report with the release URL and local checksum output.
