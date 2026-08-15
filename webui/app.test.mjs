import assert from "node:assert/strict";
import test from "node:test";

import {
  displayFrameUrl,
  focusRangeAtTime,
  nearestEvent,
  sourceFilterValue,
  trackForEvent,
  wireSelectableEvidenceRow,
  zoomedRange,
} from "./app.js";

test("an empty source selection remains an explicit empty filter", () => {
  assert.equal(sourceFilterValue(new Set(["display", "input"]), 2), null);
  assert.equal(sourceFilterValue(new Set(["input"]), 2), "input");
  assert.equal(sourceFilterValue(new Set(), 2), "");
});

test("events map into modality-neutral evidence tracks", () => {
  assert.equal(trackForEvent({ source: "performance", kind: "cpu.sample", payload: { usage: 12 } }), "cpu");
  assert.equal(trackForEvent({ source: "network", kind: "http.response", payload: { url: "https://example.test" } }), "network");
  assert.equal(trackForEvent({ source: "browser", kind: "browser.navigation", payload: { url: "https://example.test" } }), "browser");
  assert.equal(trackForEvent({ source: "console", kind: "console.message", payload: { text: "ready" } }), "logs");
  assert.equal(trackForEvent({ source: "display", kind: "display.scanout", payload: {} }), "display");
  assert.equal(trackForEvent({ source: "custom-sensor", kind: "sample", payload: {} }), "source:custom-sensor");
});

test("whole-run navigation preserves zoom while moving focus to the selected time", () => {
  assert.deepEqual(focusRangeAtTime(75, 0, 20, 0, 100), [65, 85]);
  assert.deepEqual(focusRangeAtTime(95, 0, 20, 0, 100), [80, 100]);
  assert.deepEqual(focusRangeAtTime(10, 0, 20, 0, 100), [0, 20]);
});

test("display thumbnails use reconstructed frames rather than raw artifacts", () => {
  assert.equal(displayFrameUrl(1234.7), "/api/frame-at?timeNs=1235");
});

test("lower evidence rows select by pointer, Enter, and Space", () => {
  const listeners = new Map();
  const row = { addEventListener: (type, listener) => listeners.set(type, listener) };
  const selected = [];
  wireSelectableEvidenceRow(row, "event-1", (id) => selected.push(id));
  listeners.get("click")({});
  listeners.get("keydown")({ key: "Escape", preventDefault: () => assert.fail("Escape must not activate") });
  listeners.get("keydown")({ key: "Enter", preventDefault() {} });
  listeners.get("keydown")({ key: " ", preventDefault() {} });
  assert.deepEqual(selected, ["event-1", "event-1", "event-1"]);
});

test("nearest evidence uses monotonic time on either side", () => {
  const events = [10, 20, 40].map((hostMonotonicNs) => ({ hostMonotonicNs }));
  assert.equal(nearestEvent(events, 22).hostMonotonicNs, 20);
  assert.equal(nearestEvent(events, 37).hostMonotonicNs, 40);
  assert.equal(nearestEvent([], 20), null);
});

test("zoomed ranges remain anchored and clamped to the run", () => {
  assert.deepEqual(zoomedRange(0, 100_000_000, 50_000_000, 0.5, 0, 100_000_000), [25_000_000, 75_000_000]);
  assert.deepEqual(zoomedRange(0, 50_000_000, 0, 2, 0, 100_000_000), [0, 100_000_000]);
  assert.deepEqual(zoomedRange(50_000_000, 100_000_000, 100_000_000, 2, 0, 100_000_000), [0, 100_000_000]);
});
