# Reproducible guest image

`build-base.sh` creates the one supported guest: x86-64 Ubuntu 24.04 on a Linux/KVM host. The upstream cloud image URL and SHA-256 are pinned in `ubuntu-noble-amd64.lock`; changed upstream content fails closed instead of silently changing the experiment.

Required host packages include QEMU/KVM, `cloud-image-utils`, OpenSSH client, and curl. Run:

```sh
vm/image/build-base.sh /var/lib/avm/images/noble-v1
```

The output directory contains `avm-base.qcow2`, its checksum, and a generated SSH key pair. Keep the private key outside candidate repositories. Each experiment uses a qcow2 overlay; the base is never passed writable to QEMU.

The guest provides a stock Weston desktop at 1280×720 and scale 1, Chromium with CDP on guest port 9222, SSH, deterministic locale/fonts, common development runtimes, and a `candidate` virtiofs mount at `/workspace`.
