# Architecture

## Components

| Component | Owner | Purpose |
| --- | --- | --- |
| Local Codex | User workstation | Reasons about and modifies the candidate. |
| MCP server | User workstation | Exposes a narrow AVM tool vocabulary over stdio. |
| Remote channel | Local supervisor state | Pins the project, zone, instance, run, and AVM executable. |
| AVM supervisor | Linux/KVM host | Owns QEMU, input, sensors, timeline, and artifacts. |
| Nested guest | QEMU | Runs the candidate against `/workspace`. |
| Candidate copy | Guest-visible virtiofs mount | The only project content published into the guest. |

## Trust boundary

Candidate code and the nested guest are untrusted. They must not receive:

- Codex or ChatGPT authentication;
- GCE credentials or local `gcloud` state;
- evaluator tests, hidden fault profiles, or policy configuration;
- the supervisor's SQLite timeline or immutable artifact store;
- a writable path to the original local candidate.

AVM fingerprints the candidate before remote publication. Supervisor state must
be outside the candidate directory, and evaluator-private services run on the
host rather than inside `/workspace`.

## Observation path

QEMU D-Bus display messages reconstruct full scanouts and rectangular updates.
Host monotonic timestamps are recorded before input is sent. CDP contributes
browser DOM, accessibility, console, network, trace, and screenshot events.
AT-SPI arrives through an isolated virtio-serial channel. All normalized events
join one canonical SQLite timeline; larger payloads live in a content-addressed
SHA-256 artifact store.

Direct observations, deterministic derivations, model interpretations, and
agent claims retain distinct provenance. Model output never replaces raw
evidence.

## Reset model

AVM starts QEMU paused, records the clean internal VM snapshot without the
non-migratable virtiofs device, hot-adds the candidate mount, then resumes.
Reset restarts paused QEMU, loads that snapshot, hot-adds virtiofs again, and
resumes. Guest disk mutations do not survive reset; the externally managed
candidate and evidence remain available.
