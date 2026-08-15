import assert from "node:assert/strict";
import test from "node:test";

import {
  reloadFrameWhenSelected,
  replaceRelationPaths,
  sourceFilterValue,
} from "./app.js";

test("an empty source selection remains an explicit empty filter", () => {
  assert.equal(sourceFilterValue(new Set(["display", "input"]), 2), null);
  assert.equal(sourceFilterValue(new Set(["input"]), 2), "input");
  assert.equal(sourceFilterValue(new Set(), 2), "");
});

test("relationship redraw replaces previous paths", () => {
  const container = {
    children: [],
    replaceChildren(...children) {
      this.children = children;
    },
  };
  replaceRelationPaths(container, ["old-a", "old-b"]);
  replaceRelationPaths(container, ["current"]);
  assert.deepEqual(container.children, ["current"]);
});

test("an active Frame tab reloads after the selected event changes", () => {
  let loads = 0;
  const tab = {
    getAttribute() {
      return "true";
    },
  };
  assert.equal(reloadFrameWhenSelected(tab, () => { loads += 1; }), true);
  assert.equal(loads, 1);
});

test("an inactive Frame tab remains lazy", () => {
  let loads = 0;
  const tab = {
    getAttribute() {
      return "false";
    },
  };
  assert.equal(reloadFrameWhenSelected(tab, () => { loads += 1; }), false);
  assert.equal(loads, 0);
});
