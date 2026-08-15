# Developing AVM with AVM

AVM development dogfoods AVM by default. Every behavior-changing change is
built and exercised inside a fresh candidate workspace mounted into an AVM
guest, supervised by the latest trusted released AVM binary. UI work must be
visually inspected through AVM capture and, where relevant, correlated browser
and accessibility evidence.

## Trust separation

The stable released binary is the outer supervisor. The candidate build under
test runs inside the guest and must never supervise or attest itself. Candidate
source, build output, and application state may enter the guest; supervisor
state, VM disks, credentials, and evidence stores must remain outside it.

Use a dedicated development host/run rather than taking over an unrelated
active run. The current project convention is a separate `avm-dev-*` GCE host;
the `cando` VM and its run state are not development infrastructure.

## Required loop

1. Select a trusted released AVM supervisor and create a fresh run.
2. Publish the current tracked and non-ignored candidate workspace by content
   fingerprint.
3. Build and test the candidate inside `/workspace` in the guest.
4. Exercise the affected behavior through AVM's real input and observation
   paths. For WebUI work, inspect the rendered UI in the guest browser.
5. Review timeline, artifacts, frames, provenance, and missing evidence.
6. Run the normal repository quality gates and record remaining work in
   `backlog.md`.

Pure bootstrap work that cannot execute inside AVM—guest-image construction,
outer-supervisor provisioning, platform-specific packaging, or repairing AVM
itself when no trusted supervisor can run—is the narrow exception. Record the
reason and compensating verification in the pull request and `backlog.md`.

`backlog.md` is the canonical project work tracker. Update an existing item
instead of creating parallel TODO documents, and add discovered follow-up work
before ending a development session.
