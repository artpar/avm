# Temporal perception fixture

This evaluator-owned page creates three display sequences through ordinary pointer input:

1. a target appears after a 320 ms quiet interval;
2. one region alternates A-B-A-B-A over four updates;
3. a solid target translates 160 pixels in one update.

The hit zones themselves are non-focusable transparent elements, so input does not intentionally add a pressed/focus frame. The temporal analyzer is not given these labels or timings; it receives only the canonical input and display events plus framebuffer artifacts.
