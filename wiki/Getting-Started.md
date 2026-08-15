# Getting started

## 1. Choose the two machines

AVM's full runtime uses:

1. a local workstation where Codex and authenticated `gcloud` run; and
2. an x86-64 Ubuntu 24.04 host with KVM, often a GCE instance with nested
   virtualization enabled.

They may be the same Linux machine, but Codex does not need to run on the VM.

## 2. Install host dependencies

Install QEMU 6+, KVM, D-Bus, `virtiofsd` 1.10+, OpenSSH client,
`cloud-image-utils`, curl, Node.js 22+, Rust 1.87+, Git, and Make. Confirm that
`/dev/kvm` exists and is accessible to the account running AVM.

Build AVM from source:

```sh
git clone https://github.com/artpar/avm.git
cd avm
make setup
make check
make release
```

On the Linux host, `target/release/avm --version` should report the Cargo
package version.

## 3. Build the pinned guest

```sh
mkdir -p /var/lib/avm/images/noble-v1
vm/image/build-base.sh /var/lib/avm/images/noble-v1
```

The script verifies the pinned Ubuntu cloud image hash and creates
`avm-base.qcow2`, a checksum, and an SSH key pair. Keep the private key out of
candidate repositories.

## 4. Create and start a run

```sh
mkdir -p /srv/candidates/my-app /var/lib/avm/runs

RUN_CONFIG=$(target/release/avm create-run \
  --base-image /var/lib/avm/images/noble-v1/avm-base.qcow2 \
  --candidate /srv/candidates/my-app \
  --state-root /var/lib/avm/runs)

target/release/avm start --run "$RUN_CONFIG"
target/release/avm status --run "$RUN_CONFIG"
```

`RUN_CONFIG` is the path to the generated `run.json`. Preserve the path; most
commands use it.

## 5. Prove display and input

```sh
target/release/avm capture --run "$RUN_CONFIG" --output /tmp/avm.png
AVM_BIN=target/release/avm scripts/linux-smoke.sh "$RUN_CONFIG"
```

The smoke check succeeds only after QEMU provides a real framebuffer and the
guest repaints after injected input. It also tests a drag with a display update
between pointer-down and pointer-up.

## 6. Finish safely

```sh
target/release/avm stop --run "$RUN_CONFIG"
```

Stop nested QEMU and any outer GCE instance when idle. A run is bound to the
outer host boot; after rebooting the host, create a new run instead of appending
to the old timeline.

Continue with [[Operations]] or connect a local agent using [[Codex and MCP]].
