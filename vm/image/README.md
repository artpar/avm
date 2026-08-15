# Reproducible guest image

`build-base.sh` creates the one supported guest: x86-64 Ubuntu 24.04 on a Linux/KVM host. The upstream cloud image URL and SHA-256 are pinned in `ubuntu-noble-amd64.lock`; changed upstream content fails closed instead of silently changing the experiment.

Required host packages include QEMU/KVM, `cloud-image-utils`, OpenSSH client, and curl. Run:

```sh
vm/image/build-base.sh /var/lib/avm/images/noble-v1
```

Provisioning is allowed up to 30 minutes by default so that snap and package
downloads can complete on variable networks. Set
`AVM_IMAGE_PROVISION_TIMEOUT_SECONDS` to another positive number of seconds when
the host needs a different operational bound. A timeout reports the current
`cloud-init status --long` plus the tail of both guest serial and QEMU logs;
completed cloud-init error states fail the build with the same diagnostics.

The output directory contains `avm-base.qcow2`, its checksum, and a generated SSH key pair. Keep the private key outside candidate repositories. Each experiment uses a qcow2 overlay; the base is never passed writable to QEMU.

The guest provides a stock Weston desktop at 1280×720 and scale 1, Chromium with CDP on guest port 9222, software WebGL2, SSH, deterministic locale/fonts, common development runtimes, XFCE Terminal, Galculator, ALSA tooling with an Intel HDA guest device, and a `candidate` virtiofs mount at `/workspace`.

WebGL2 uses Chromium's explicit SwiftShader fallback while QEMU remains on the
2D `virtio-vga` and shared-memory D-Bus scanout path. This preserves AVM's
authoritative framebuffer capture on GPU-less GCE hosts. Chromium labels this
fallback unsafe because shader JIT code runs with reduced GPU-process security;
AVM confines it to the disposable, untrusted guest and never enables host GL or
DMA-BUF capture. Run `avm-linux-webgl-smoke RUN_CONFIG` to qualify the browser,
known framebuffer colors, and an input-driven repaint.

An evaluator-owned AT-SPI sensor starts before desktop applications and exposes bounded semantic trees and events through the named virtio port `org.avm.accessibility`. The host initiates each observation with a protocol handshake, causing a fresh tree snapshot without restarting the sensor or its accessibility bus. The guest can write only to the scoped virtio port; it does not own or mount the host evidence store.
