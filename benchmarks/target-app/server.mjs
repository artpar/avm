import http from 'node:http';
import { readFile, writeFile, rename, mkdir } from 'node:fs/promises';
import { dirname, extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';
import crypto from 'node:crypto';

const root = dirname(fileURLToPath(import.meta.url));
const port = Number(process.env.PORT || 3000);
const statePath = process.env.BOARD_STATE || join(root, '.data', 'board.json');
const latencyMs = Number(process.env.BOARD_LATENCY_MS || 120);
const mime = { '.html': 'text/html', '.css': 'text/css', '.js': 'text/javascript' };

function seed() {
  return {
    revision: 1,
    projects: [
      { id: 'atlas', name: 'Atlas launch' },
      { id: 'relay', name: 'Relay reliability' }
    ],
    columns: [
      { id: 'backlog', name: 'Backlog' },
      { id: 'active', name: 'In progress' },
      { id: 'done', name: 'Done' }
    ],
    cards: [
      { id: 'a1', projectId: 'atlas', columnId: 'backlog', title: 'Review empty state', description: 'Make first-run guidance clear.', priority: 'medium' },
      { id: 'a2', projectId: 'atlas', columnId: 'active', title: 'Ship keyboard map', description: 'Document shortcuts in the command menu.', priority: 'high' },
      { id: 'r1', projectId: 'relay', columnId: 'backlog', title: 'Retry interrupted upload', description: 'Resume at the last confirmed chunk.', priority: 'high' }
    ],
    history: []
  };
}

async function load() {
  try { return JSON.parse(await readFile(statePath, 'utf8')); }
  catch (error) {
    if (error.code !== 'ENOENT') throw error;
    const state = seed();
    await save(state);
    return state;
  }
}

async function save(state) {
  await mkdir(dirname(statePath), { recursive: true });
  const temporary = `${statePath}.${process.pid}.tmp`;
  await writeFile(temporary, `${JSON.stringify(state, null, 2)}\n`);
  await rename(temporary, statePath);
}

function json(response, status, body) {
  response.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' });
  response.end(JSON.stringify(body));
}

async function body(request) {
  const chunks = [];
  let size = 0;
  for await (const chunk of request) {
    size += chunk.length;
    if (size > 1_000_000) throw Object.assign(new Error('body too large'), { status: 413 });
    chunks.push(chunk);
  }
  return JSON.parse(Buffer.concat(chunks).toString('utf8') || '{}');
}

function snapshot(state) {
  const { history, ...publicState } = state;
  return structuredClone(publicState);
}

async function mutate(response, operation) {
  const state = await load();
  const before = snapshot(state);
  const result = operation(state);
  state.history.push(before);
  state.history = state.history.slice(-20);
  state.revision += 1;
  await new Promise(resolve => setTimeout(resolve, latencyMs));
  await save(state);
  json(response, 200, { ...result, revision: state.revision });
}

const server = http.createServer(async (request, response) => {
  try {
    const url = new URL(request.url, 'http://localhost');
    if (request.method === 'GET' && url.pathname === '/api/state') {
      return json(response, 200, snapshot(await load()));
    }
    if (request.method === 'POST' && url.pathname === '/api/cards') {
      const input = await body(request);
      return mutate(response, state => {
        if (!input.title?.trim() || !state.projects.some(project => project.id === input.projectId)) {
          throw Object.assign(new Error('invalid card'), { status: 400 });
        }
        const card = { id: crypto.randomUUID(), projectId: input.projectId, columnId: 'backlog', title: input.title.trim(), description: input.description?.trim() || '', priority: input.priority || 'medium' };
        state.cards.push(card);
        return { card };
      });
    }
    const cardMatch = url.pathname.match(/^\/api\/cards\/([^/]+)$/);
    if (request.method === 'PATCH' && cardMatch) {
      const input = await body(request);
      return mutate(response, state => {
        const card = state.cards.find(candidate => candidate.id === cardMatch[1]);
        if (!card) throw Object.assign(new Error('card not found'), { status: 404 });
        for (const key of ['title', 'description', 'priority', 'columnId']) {
          if (input[key] !== undefined) card[key] = input[key];
        }
        return { card };
      });
    }
    if (request.method === 'POST' && url.pathname === '/api/undo') {
      const state = await load();
      const previous = state.history.pop();
      if (!previous) return json(response, 409, { error: 'nothing to undo' });
      const history = state.history;
      Object.assign(state, previous, { history, revision: state.revision + 1 });
      await save(state);
      return json(response, 200, snapshot(state));
    }
    if (request.method !== 'GET') return json(response, 404, { error: 'not found' });
    const requested = url.pathname === '/' ? 'index.html' : url.pathname.slice(1);
    const path = normalize(join(root, requested));
    if (!path.startsWith(`${root}/`)) return json(response, 403, { error: 'forbidden' });
    const content = await readFile(path);
    response.writeHead(200, { 'content-type': mime[extname(path)] || 'application/octet-stream' });
    response.end(content);
  } catch (error) {
    json(response, error.status || 500, { error: error.message || 'internal error' });
  }
});

server.listen(port, '0.0.0.0', () => console.log(`project board listening on ${port}`));
