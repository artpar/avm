# Product

## Register

product

## Users

AVM serves software developers who use an agent to build, run, inspect, and
verify their own software inside an instrumented virtual computer. They need to
understand the complete recorded machine history and move from an observed
defect to trustworthy evidence without reconstructing the story from
disconnected logs and screenshots.

## Product Purpose

AVM makes graphical software-development work observable, reproducible, and
verifiable while keeping the supervisor—not the candidate—authoritative. The
human interface should let a developer see what the guest and agent experienced,
navigate the unified timeline, and inspect supporting evidence. It is
deliberately observational: controls remain explicit CLI/MCP operations outside
the human interface.

Success means developers can use AVM as the default environment for developing
their own software, including AVM itself, without needing to memorize its CLI
or compromise its trust boundary.

## Brand Personality

Minimal, dense, semantically intuitive. The interface should feel precise and
quiet under sustained technical use. Its voice is direct, factual, and explicit
about provenance, uncertainty, destructive actions, and system state.

## Anti-references

Avoid decorative color, ornamental dashboards, oversized marketing metrics,
card-grid home screens, glass effects, gratuitous animation, and interfaces
that hide operational detail behind vague status labels. The product is
strictly grayscale; meaning must never depend on hue.

## Design Principles

1. **Show the machine truth.** Keep the live guest, recorded history, agent
   actions, and evidence visibly connected.
2. **Make provenance legible.** Distinguish raw observation, deterministic
   derivation, model interpretation, and agent claim wherever they appear.
3. **Optimize for practiced use.** Support keyboard-first navigation, compact
   density, stable spatial relationships, and progressive disclosure.
4. **Keep the boundary visible.** Clearly separate trusted supervisor state,
   local candidate state, the published fingerprint, and guest-visible state.
5. **Prefer recovery to ceremony.** Confirm materially destructive operations,
   preserve inspectable receipts, and make ordinary reversible actions fast.

## Accessibility & Inclusion

Target WCAG 2.2 AA. All workflows must be keyboard operable, expose useful
screen-reader semantics, respect reduced-motion preferences, remain usable at
200% zoom, and provide non-color cues for every state. Grayscale contrast,
shape, text, iconography, and position carry meaning; color is not introduced
as an optional semantic shortcut.
