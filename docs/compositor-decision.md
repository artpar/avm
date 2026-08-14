# Compositor integration decision

Decision: do not fork or embed Weston for the current experiment.

The existing layers supplied the evidence needed by every failure observed so far:

- QEMU D-Bus scanouts and updates supplied authoritative pixels, damage rectangles, cursor state, and host timestamps.
- CDP supplied browser DOM, accessibility, network, console, performance, and trace data correlated to those pixels.
- The guest AT-SPI sensor supplied native application identity, roles, states, actions, text/value interfaces, geometry, focus, window lifecycle, and reconnectable trees.
- Temporal analysis recovered delayed response, no response, repetition, reversion, flicker, and exact pixel translation without compositor changes.

The in-place Weston restart used during sensor development invalidated the active QEMU input path until a fresh VM boot. That was a test-setup lifecycle issue; it did not prevent observation in clean runs and is not evidence that compositor internals are missing.

Information still unavailable includes authoritative Wayland surface IDs, surface-level damage ownership, compositor focus transitions independent of AT-SPI, and output presentation timestamps. None has materially blocked diagnosis or evaluation yet. Adding libweston instrumentation would increase the trusted code and maintenance surface without closing a demonstrated observation gap.

Reconsider this decision only when a reproducible evaluator failure cannot be distinguished using pixels plus CDP/AT-SPI/runtime evidence and the competing hypotheses differ specifically in surface lifecycle, compositor focus, or presentation timing. Record that failure first, then add the narrowest Weston timeline/debug instrumentation capable of discriminating it.
