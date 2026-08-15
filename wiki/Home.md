# AVM wiki

AVM is a host-owned, instrumented virtual computer for software-development
agents. A trusted supervisor operates and observes a nested Linux guest while
keeping credentials, evidence, and evaluator state outside the candidate
workspace.

## Start here

- [[Getting Started]] — install AVM, build the guest, and start a run.
- [[Architecture]] — understand hosts, guests, trust boundaries, and evidence.
- [[Operations]] — operate, inspect, reset, and clean up runs.
- [[Codex and MCP]] — connect local Codex to a remote GCE/KVM run.
- [[Troubleshooting]] — diagnose common host, display, tunnel, and browser issues.
- [[Releasing]] — versioning, release PRs, artifacts, and verification.

The run WebUI starts automatically with `avm start`. It is a grayscale,
read-only semantic timeline and artifact inspector; operational control remains
in explicit CLI or MCP calls.

The source of this wiki is versioned in the repository's `wiki/` directory and
synchronized to GitHub Wiki after changes land on `main`.

## Current scope

AVM targets an x86-64 Ubuntu 24.04 KVM host and an Ubuntu 24.04 nested graphical
guest. It is a pre-1.0 research system with a completed scoped acceptance audit,
not a hardened multi-tenant VM service.
