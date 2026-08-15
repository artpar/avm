import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const source = await readFile(new URL('./app.js', import.meta.url), 'utf8');
const html = await readFile(new URL('./index.html', import.meta.url), 'utf8');

test('fixture requires WebGL2 and exposes browser-observable outcomes', () => {
  assert.match(source, /getContext\('webgl2'/);
  assert.match(source, /AVM_WEBGL2_FAIL/);
  assert.match(source, /AVM_WEBGL2_OK/);
  assert.match(source, /AVM_WEBGL2_UPDATED/);
  assert.match(source, /gl\.clear\(gl\.COLOR_BUFFER_BIT\)/);
  assert.match(html, /<canvas id="surface"><\/canvas>/);
});

test('fixture uses stable framebuffer acceptance colors', () => {
  assert.match(source, /gl\.clearColor\(32 \/ 255, 191 \/ 255, 64 \/ 255, 1\)/);
  assert.match(source, /gl\.clearColor\(230 \/ 255, 51 \/ 255, 204 \/ 255, 1\)/);
});
