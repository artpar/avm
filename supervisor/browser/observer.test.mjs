import test from "node:test";
import assert from "node:assert/strict";

import { parseArguments, unverifiedWindowEstimate } from "./observer.mjs";

test("parses required observer options", () => {
  assert.deepEqual(
    parseArguments([
      "--endpoint",
      "http://127.0.0.1:9222",
      "--trace",
      "/tmp/trace.zip",
      "--artifacts-dir",
      "/tmp/artifacts",
      "--duration-ms",
      "250",
    ]),
    {
      endpoint: "http://127.0.0.1:9222",
      trace: "/tmp/trace.zip",
      artifactsDir: "/tmp/artifacts",
      durationMs: 250,
    },
  );
});

test("labels the window-metric mapping as an unverified estimate", () => {
  const mapping = unverifiedWindowEstimate({
    screenX: 118,
    screenY: 42,
    outerWidth: 1020,
    outerHeight: 738,
    innerWidth: 1018,
    innerHeight: 650,
    devicePixelRatio: 1,
    visualViewport: { offsetLeft: 0, offsetTop: 0, scale: 1 },
  });
  assert.deepEqual(mapping.displayContentOrigin, { x: 119, y: 130 });
  assert.equal(mapping.viewport.width, 1018);
  assert.equal(mapping.verified, false);
  assert.equal(mapping.authority, "browser_window_metrics_only");
});
