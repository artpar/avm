# AVM interface system

AVM is a dense, read-only instrument panel for understanding what an agent and
its graphical computer experienced. Its interface is evidence, not decoration:
every mark should identify an event, artifact, relationship, boundary, or gap.

## Visual language

- Grayscale only. Meaning must never depend on hue.
- System sans for interface copy; system monospace for time, IDs, hashes,
  source names, and payloads.
- A compact 4 px spacing rhythm with clear section boundaries and restrained
  use of borders.
- Solid dark selection, hatched inferred state, outline-only recorded state,
  and dotted unknown or missing state.
- No ornamental gradients, shadows, glass effects, illustrations, or motion.
- Minimum 4.5:1 text contrast and visible keyboard focus. Respect reduced
  motion and preserve useful layouts at 200% zoom.

## Interaction language

The primary workspace is a causal trace. A switch exposes the same evidence as
an event table. Selecting an event opens the frame/artifact inspector without
leaving the timeline. Filters change what is shown; they never change the run.

The interface has no VM, input, policy, deletion, or artifact mutation controls.
It labels raw observation, deterministic derivation, model interpretation, and
agent claim explicitly and never draws a causal edge without a recorded basis.

Keyboard conventions: `/` focuses search, `T` selects trace, `A` selects the
event table, `P` toggles the evidence panel, and arrow keys move the selection.

## Product test

A user should be able to answer three questions without consulting raw files:

1. What happened, and in what order?
2. What evidence supports this event or relationship?
3. Where is evidence absent, truncated, derived, or uncertain?

See [docs/human-interface.md](docs/human-interface.md) for product behavior and
[docs/webui-architecture.md](docs/webui-architecture.md) for trust boundaries.
