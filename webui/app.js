const PROVENANCE = ["observed", "derived", "model_interpreted", "agent_claim"];
const TRACKS = [
  { id: "cpu", label: "CPU", glyph: "P" },
  { id: "disk", label: "Disk", glyph: "D" },
  { id: "network", label: "Network", glyph: "N" },
  { id: "logs", label: "Logs", glyph: "L" },
  { id: "browser", label: "Browser", glyph: "B" },
  { id: "input", label: "Input", glyph: "I" },
  { id: "display", label: "Display", glyph: "S" },
  { id: "audio", label: "Audio", glyph: "A" },
  { id: "vm", label: "VM", glyph: "V" },
];

const state = {
  run: null,
  overview: null,
  events: [],
  relations: [],
  activeSources: new Set(),
  activeProvenance: new Set(PROVENANCE),
  selectedId: null,
  cursorNs: null,
  focusStartNs: null,
  focusEndNs: null,
  perspective: "trace",
  query: "",
  abort: null,
  pendingEventId: null,
  drag: null,
  problem: null,
  frameToken: 0,
};

const $ = (selector) => document.querySelector(selector);
const $$ = (selector) => [...document.querySelectorAll(selector)];

export function sourceFilterValue(activeSources, totalSources) {
  if (activeSources.size === totalSources) return null;
  return [...activeSources].sort().join(",");
}

export function trackForEvent(event) {
  const source = String(event.source || "").toLowerCase();
  const kind = String(event.kind || "").toLowerCase();
  const searchable = `${source} ${kind} ${JSON.stringify(event.payload || {})}`.toLowerCase();
  // Collector identity is authoritative. Payload keywords only classify
  // collectors that do not already have a canonical semantic lane.
  if (source === "browser") return "browser";
  if (source === "network") return "network";
  if (source === "display") return "display";
  if (source === "audio") return "audio";
  if (["input", "cursor"].includes(source)) return "input";
  if (source === "performance") return "cpu";
  if (["console", "log", "runtime"].includes(source)) return "logs";
  if (["vm", "transport", "policy", "temporal"].includes(source)) return "vm";
  if (/\b(browser|page|navigation|dom|accessibility)\b/.test(searchable)) return "browser";
  if (/\b(network|http|https|request|response|xhr|fetch|websocket|dns|tcp)\b/.test(searchable)) return "network";
  if (/\b(file|filesystem|disk|fs\.|io\.|read|write|download|upload)\b/.test(searchable)) return "disk";
  if (/\b(log|console|stderr|stdout|warning|exception|error)\b/.test(searchable)) return "logs";
  if (/\b(cpu|profile|metric|performance|memory)\b/.test(searchable)) return "cpu";
  if (/\b(pointer|mouse|keyboard|key\.|scroll|click|touch)\b/.test(searchable)) return "input";
  if (/\b(display|scanout|frame|screen)\b/.test(searchable)) return "display";
  if (/\b(audio|pcm|sound|speaker|microphone)\b/.test(searchable)) return "audio";
  if (/\b(vm|qemu|snapshot|reset|boot|shutdown|workspace)\b/.test(searchable)) return "vm";
  return `source:${source || "unknown"}`;
}

export function nearestEvent(events, timeNs) {
  if (!events.length || !Number.isFinite(timeNs)) return null;
  let low = 0;
  let high = events.length - 1;
  while (low < high) {
    const middle = Math.floor((low + high) / 2);
    if (events[middle].hostMonotonicNs < timeNs) low = middle + 1;
    else high = middle;
  }
  const after = events[low];
  const before = events[Math.max(0, low - 1)];
  return Math.abs(after.hostMonotonicNs - timeNs) < Math.abs(before.hostMonotonicNs - timeNs) ? after : before;
}

export function zoomedRange(startNs, endNs, anchorNs, factor, runStartNs, runEndNs) {
  const runSpan = Math.max(1, runEndNs - runStartNs);
  const span = Math.min(runSpan, Math.max(1_000_000, (endNs - startNs) * factor));
  const ratio = endNs === startNs ? 0.5 : (anchorNs - startNs) / (endNs - startNs);
  let nextStart = anchorNs - span * ratio;
  let nextEnd = nextStart + span;
  if (nextStart < runStartNs) { nextEnd += runStartNs - nextStart; nextStart = runStartNs; }
  if (nextEnd > runEndNs) { nextStart -= nextEnd - runEndNs; nextEnd = runEndNs; }
  return [Math.max(runStartNs, Math.round(nextStart)), Math.min(runEndNs, Math.round(nextEnd))];
}

export function focusRangeAtTime(timeNs, focusStartNs, focusEndNs, runStartNs, runEndNs) {
  if (timeNs >= focusStartNs && timeNs <= focusEndNs) return [focusStartNs, focusEndNs];
  const span = Math.min(Math.max(1, focusEndNs - focusStartNs), Math.max(1, runEndNs - runStartNs));
  let start = timeNs - span / 2;
  let end = start + span;
  if (start < runStartNs) { end += runStartNs - start; start = runStartNs; }
  if (end > runEndNs) { start -= end - runEndNs; end = runEndNs; }
  return [Math.max(runStartNs, Math.round(start)), Math.min(runEndNs, Math.round(end))];
}

export function displayFrameUrl(timeNs) {
  return `/api/frame-at?timeNs=${Math.round(timeNs)}`;
}

async function api(path, options = {}) {
  const response = await fetch(path, { ...options, headers: { Accept: "application/json", ...options.headers } });
  if (!response.ok) {
    const problem = await response.json().catch(() => ({
      code: "http_request_failed",
      message: response.statusText || "Evidence request failed.",
    }));
    const error = new Error(problem.message || `Evidence request failed (${response.status})`);
    error.problem = { ...problem, status: response.status, path };
    throw error;
  }
  return response.json();
}

function el(tag, attributes = {}, children = []) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attributes)) {
    if (key === "class") node.className = value;
    else if (key === "text") node.textContent = value;
    else if (key.startsWith("data-")) node.setAttribute(key, value);
    else if (key === "checked") node.checked = Boolean(value);
    else if (value !== null && value !== undefined) node.setAttribute(key, value);
  }
  for (const child of Array.isArray(children) ? children : [children]) if (child) node.append(child);
  return node;
}

function formatCount(value) { return new Intl.NumberFormat().format(value ?? 0); }
function formatDuration(ns) {
  if (!Number.isFinite(ns)) return "—";
  const absolute = Math.abs(ns);
  if (absolute < 1_000) return `${Math.round(ns)} ns`;
  if (absolute < 1_000_000) return `${(ns / 1_000).toFixed(1)} μs`;
  if (absolute < 1_000_000_000) return `${(ns / 1_000_000).toFixed(1)} ms`;
  if (absolute < 60_000_000_000) return `${(ns / 1_000_000_000).toFixed(3)} s`;
  const seconds = Math.floor(absolute / 1_000_000_000);
  const sign = ns < 0 ? "−" : "";
  return `${sign}${Math.floor(seconds / 60)}m ${String(seconds % 60).padStart(2, "0")}s`;
}
function formatWallTime(event) {
  const date = new Date(event?.wallClockTime);
  if (!event || Number.isNaN(date.valueOf())) return event?.wallClockTime || "—";
  return `${date.toISOString().slice(11, 23)} UTC`;
}
function relativeTime(ns) { return `+${formatDuration(ns - (state.run?.startNs || ns))}`; }
function shortId(value, size = 8) { return value ? `${value.slice(0, size)}…${value.slice(-4)}` : "—"; }
function provenanceLabel(value) {
  return ({ observed: "Raw", derived: "Derived", model_interpreted: "Model", agent_claim: "Claim" })[value] || value;
}
function eventSummary(event) {
  const payload = event?.payload || {};
  const preferred = [payload.summary, payload.message, payload.body, payload.url, payload.path, payload.text, payload.status, payload.error, payload.key];
  const value = preferred.find((item) => item !== undefined && item !== null && String(item).trim());
  if (value !== undefined) return String(value);
  const entries = Object.entries(payload).slice(0, 3).map(([key, item]) => `${key}=${typeof item === "object" ? "…" : item}`);
  return entries.join(" · ") || "No payload fields";
}
function scale(value, start, end) { return end <= start ? 0 : Math.max(0, Math.min(1, (value - start) / (end - start))); }
function focusSpan() { return Math.max(1, state.focusEndNs - state.focusStartNs); }

async function initialize() {
  bindControls();
  restoreUrlState();
  try {
    const [run, overview] = await Promise.all([api("/api/run"), api("/api/overview?buckets=480")]);
    state.run = run;
    state.overview = overview;
    state.activeSources = new Set(Object.keys(run.sourceCounts));
    const runStart = run.startNs ?? 0;
    const runEnd = run.endNs ?? runStart;
    // An exact event deep link takes precedence over a stale focus window. The
    // event is resolved from the whole run, then subsequent interactions keep
    // the ordinary synchronized focus semantics.
    state.focusStartNs = state.pendingEventId ? runStart : (clampUrlNumber("start", runStart, runEnd) ?? runStart);
    state.focusEndNs = state.pendingEventId ? runEnd : (clampUrlNumber("end", state.focusStartNs, runEnd) ?? runEnd);
    state.cursorNs = clampUrlNumber("time", runStart, runEnd);
    renderRunChrome();
    renderFilters();
    await loadEvents();
  } catch (error) {
    showFatal(error);
  }
}

function restoreUrlState() {
  const params = new URLSearchParams(location.search);
  state.perspective = params.get("view") === "table" ? "table" : "trace";
  state.pendingEventId = params.get("event");
  state.query = params.get("query") || "";
  if ($("#query")) $("#query").value = state.query;
}

function clampUrlNumber(name, minimum, maximum) {
  const value = Number(new URLSearchParams(location.search).get(name));
  return Number.isFinite(value) && value >= minimum && value <= maximum ? value : null;
}

function renderRunChrome() {
  const run = state.run;
  $("#run-identity").textContent = `Run ${shortId(run.runId)} · ${formatCount(run.eventCount)} evidence records · ${formatCount(Object.keys(run.sourceCounts).length)} sources`;
  $("#overview-duration").textContent = formatDuration((run.endNs || 0) - (run.startNs || 0));
  $("#coverage-status").textContent = `${formatCount(run.artifactCount)} artifacts · ${run.width}×${run.height} display`;
  setPerspective(state.perspective, false);
}

function renderFilters() {
  const sourceRoot = $("#source-filters");
  sourceRoot.replaceChildren();
  for (const [source, count] of Object.entries(state.run.sourceCounts).sort(([a], [b]) => a.localeCompare(b))) {
    const input = el("input", { type: "checkbox", checked: true, "data-source-filter": source });
    input.addEventListener("change", () => {
      if (input.checked) state.activeSources.add(source); else state.activeSources.delete(source);
      loadEvents();
    });
    sourceRoot.append(el("label", {}, [input, el("span", { text: source }), el("small", { class: "mono", text: formatCount(count) })]));
  }
  const provenanceRoot = $("#provenance-filters");
  provenanceRoot.replaceChildren();
  for (const value of PROVENANCE) {
    const count = state.run.provenanceCounts[value] || 0;
    const input = el("input", { type: "checkbox", checked: true, "data-provenance-filter": value });
    input.addEventListener("change", () => {
      if (input.checked) state.activeProvenance.add(value); else state.activeProvenance.delete(value);
      loadEvents();
    });
    provenanceRoot.append(el("label", {}, [input, el("span", { text: provenanceLabel(value) }), el("small", { class: "mono", text: formatCount(count) })]));
  }
}

async function loadEvents() {
  state.abort?.abort();
  state.abort = new AbortController();
  $("#result-state").textContent = "Loading focus range…";
  if (state.activeSources.size === 0 || state.activeProvenance.size === 0) {
    state.events = [];
    state.relations = [];
    renderWorkspace();
    $("#result-state").textContent = "0 visible records";
    return;
  }
  const params = new URLSearchParams({
    startNs: String(Math.round(state.focusStartNs)),
    endNs: String(Math.round(state.focusEndNs)),
    limit: "20000",
  });
  if (state.query) params.set("query", state.query);
  const sourceFilter = sourceFilterValue(state.activeSources, Object.keys(state.run.sourceCounts).length);
  if (sourceFilter !== null) params.set("source", sourceFilter);
  if (state.activeProvenance.size !== PROVENANCE.length) params.set("provenance", [...state.activeProvenance].join(","));
  try {
    const result = await api(`/api/events?${params}`, { signal: state.abort.signal });
    state.events = result.events;
    state.relations = result.relations;
    if (state.pendingEventId && state.events.some((event) => event.id === state.pendingEventId)) {
      const event = state.events.find((candidate) => candidate.id === state.pendingEventId);
      state.selectedId = event.id;
      state.cursorNs = event.hostMonotonicNs;
      state.pendingEventId = null;
    } else if (state.selectedId && !state.events.some((event) => event.id === state.selectedId)) {
      state.selectedId = null;
    }
    if (!Number.isFinite(state.cursorNs) && state.events.length) {
      const initial = defaultEvent(state.events);
      state.selectedId = initial.id;
      state.cursorNs = initial.hostMonotonicNs;
    }
    renderWorkspace();
    $("#result-state").textContent = result.truncated
      ? `${formatCount(result.events.length)} of ${formatCount(result.totalBeforeLimit)} records in focus`
      : `${formatCount(result.events.length)} records in focus`;
    if (result.truncated) $("#coverage-status").textContent = "Focus result truncated · zoom in for complete detail";
    clearProblem();
  } catch (error) {
    if (error.name === "AbortError") return;
    showProblem(error, "Timeline request failed");
    $("#result-state").textContent = "Evidence request failed";
  }
}

function defaultEvent(events) {
  const highSignal = [...events].reverse().find((event) => /error|failed|failure|exception|panic|crash/i.test(`${event.kind} ${eventSummary(event)}`));
  return highSignal || events[events.length - 1];
}

function renderWorkspace() {
  renderFocusReadout();
  renderOverview();
  renderRuler();
  renderTracks();
  renderTable();
  renderEvidenceCanvas();
  const empty = state.events.length === 0;
  $("#trace-empty").hidden = !empty;
  $("#track-list").hidden = empty;
  $("#time-ruler").hidden = empty;
  $("#trace-view").hidden = state.perspective !== "trace";
  $("#table-view").hidden = state.perspective !== "table";
  updateUrl();
}

function renderFocusReadout() {
  const whole = state.focusStartNs === state.run.startNs && state.focusEndNs === state.run.endNs;
  $("#focus-range").textContent = whole
    ? `Whole run · ${formatDuration(focusSpan())}`
    : `${relativeTime(state.focusStartNs)} — ${relativeTime(state.focusEndNs)} · ${formatDuration(focusSpan())}`;
}

function prepareCanvas(canvas) {
  const rect = canvas.getBoundingClientRect();
  const ratio = window.devicePixelRatio || 1;
  const width = Math.max(1, Math.round(rect.width * ratio));
  const height = Math.max(1, Math.round(rect.height * ratio));
  if (canvas.width !== width || canvas.height !== height) { canvas.width = width; canvas.height = height; }
  const context = canvas.getContext("2d");
  context.setTransform(ratio, 0, 0, ratio, 0, 0);
  context.clearRect(0, 0, rect.width, rect.height);
  return { context, width: rect.width, height: rect.height };
}

function renderOverview() {
  const canvas = $("#overview-canvas");
  const { context, width, height } = prepareCanvas(canvas);
  const buckets = state.overview?.buckets || [];
  const maximum = Math.max(1, ...buckets.map((bucket) => bucket.count));
  context.fillStyle = "#252525";
  buckets.forEach((bucket, index) => {
    const barWidth = Math.max(1, width / buckets.length);
    const barHeight = Math.max(1, (bucket.count / maximum) * (height - 4));
    context.fillRect(index * barWidth, height - barHeight, Math.ceil(barWidth), barHeight);
  });
  const runStart = state.run.startNs || 0;
  const runEnd = state.run.endNs || runStart + 1;
  const focusLeft = scale(state.focusStartNs, runStart, runEnd) * 100;
  const focusRight = scale(state.focusEndNs, runStart, runEnd) * 100;
  const focusWindow = $("#focus-window");
  focusWindow.style.left = `${focusLeft}%`;
  focusWindow.style.width = `${Math.max(0.2, focusRight - focusLeft)}%`;
  positionCursor($("#overview-cursor"), state.cursorNs, runStart, runEnd);
}

function renderRuler() {
  const ruler = $("#time-ruler");
  ruler.replaceChildren();
  for (let index = 0; index <= 6; index++) {
    const time = state.focusStartNs + (focusSpan() * index) / 6;
    const tick = el("span", { class: "ruler-tick mono", text: relativeTime(time) });
    tick.style.left = `${(index / 6) * 100}%`;
    ruler.append(tick);
  }
}

function renderTracks() {
  const root = $("#track-list");
  root.replaceChildren();
  const grouped = groupBy(state.events, trackForEvent);
  const known = new Set(TRACKS.map((track) => track.id));
  const descriptors = [...TRACKS];
  for (const trackId of grouped.keys()) {
    if (!known.has(trackId)) descriptors.push({ id: trackId, label: trackId.replace(/^source:/, ""), glyph: "·" });
  }
  for (const descriptor of descriptors) {
    const events = grouped.get(descriptor.id) || [];
    const row = el("section", { class: "track-row", "data-track": descriptor.id, "aria-label": `${descriptor.label} evidence track` });
    row.append(el("header", { class: "track-header" }, [
      el("span", { class: "track-symbol", "aria-hidden": "true", text: descriptor.glyph }),
      el("span", { class: "track-name", text: descriptor.label, title: descriptor.label }),
      el("span", { class: "track-count mono", text: formatCount(events.length) }),
    ]));
    const surface = el("button", {
      type: "button",
      class: "track-surface",
      "data-track-surface": descriptor.id,
      "aria-label": `${descriptor.label}. ${formatCount(events.length)} records in the focus range. Click to select time; drag to focus.`,
    });
    const canvas = el("canvas", { "aria-hidden": "true" });
    surface.append(canvas);
    const cursor = el("span", { class: "time-cursor", "aria-hidden": "true" });
    positionCursor(cursor, state.cursorNs, state.focusStartNs, state.focusEndNs);
    surface.append(cursor);
    if (!events.length) surface.append(el("span", { class: "track-state", text: trackCollectionState(descriptor.id) }));
    attachTimeSurface(surface, events, state.focusStartNs, state.focusEndNs);
    row.append(surface);
    root.append(row);
    drawTrack(canvas, descriptor.id, events);
    if (descriptor.id === "display") renderDisplayThumbnails(surface, events);
  }
}

function groupBy(items, key) {
  const groups = new Map();
  for (const item of items) {
    const value = key(item);
    if (!groups.has(value)) groups.set(value, []);
    groups.get(value).push(item);
  }
  return groups;
}

function trackCollectionState(trackId) {
  return trackCoverage(trackId) ? "No matching evidence in this focus range" : "Not collected";
}

function drawTrack(canvas, trackId, events) {
  const { context, width, height } = prepareCanvas(canvas);
  const coverage = trackCoverage(trackId);
  if (coverage) {
    const left = scale(coverage.startNs, state.focusStartNs, state.focusEndNs) * width;
    const right = scale(coverage.endNs, state.focusStartNs, state.focusEndNs) * width;
    if (right > 0 && left < width) {
      context.fillStyle = "#b8b8b8";
      context.fillRect(Math.max(0, left), height - 2, Math.max(1, Math.min(width, right) - Math.max(0, left)), 1);
    }
  }
  if (!events.length) return;
  const values = trackId === "cpu" ? events.map(numericValue) : [];
  const hasValues = values.some(Number.isFinite);
  context.strokeStyle = "#222";
  context.fillStyle = "#333";
  context.lineWidth = 1;
  if (hasValues) {
    const valid = values.filter(Number.isFinite);
    const maximum = Math.max(...valid, 1);
    context.beginPath();
    events.forEach((event, index) => {
      const x = scale(event.hostMonotonicNs, state.focusStartNs, state.focusEndNs) * width;
      const value = Number.isFinite(values[index]) ? values[index] : 0;
      const y = height - 3 - (value / maximum) * (height - 7);
      if (index === 0) context.moveTo(x, y); else context.lineTo(x, y);
    });
    context.stroke();
    return;
  }
  const density = new Uint16Array(Math.max(1, Math.floor(width)));
  for (const event of events) {
    const x = Math.min(density.length - 1, Math.max(0, Math.floor(scale(event.hostMonotonicNs, state.focusStartNs, state.focusEndNs) * density.length)));
    density[x] += 1;
  }
  const maximum = Math.max(1, ...density);
  density.forEach((count, x) => {
    if (!count) return;
    const markHeight = trackId === "vm" ? height - 10 : Math.max(5, (count / maximum) * (height - 8));
    if (trackId === "logs") {
      context.beginPath();
      context.arc(x, height / 2, Math.min(3, 1 + count / maximum * 2), 0, Math.PI * 2);
      context.fill();
    } else if (trackId === "network") {
      context.fillRect(x, 5 + (x % 3) * 5, Math.max(2, Math.min(14, count * 2)), 2);
    } else if (trackId === "audio") {
      context.fillRect(x, (height - markHeight) / 2, 1, markHeight);
    } else {
      context.fillRect(x, height - markHeight - 4, 1, markHeight);
    }
  });
}

function trackCoverage(trackId) {
  const spans = Object.entries(state.run.sourceCoverage || {})
    .filter(([source]) => trackForEvent({ source, kind: "", payload: {} }) === trackId)
    .map(([, coverage]) => coverage);
  if (!spans.length) return null;
  return {
    startNs: Math.min(...spans.map((coverage) => coverage.startNs)),
    endNs: Math.max(...spans.map((coverage) => coverage.endNs)),
  };
}

function numericValue(event) {
  const entries = Object.entries(event.payload || {});
  const preferred = entries.find(([key, value]) => /cpu|usage|percent|utilization/i.test(key) && Number.isFinite(Number(value)));
  if (preferred) return Number(preferred[1]);
  const any = entries.find(([, value]) => Number.isFinite(Number(value)));
  return any ? Number(any[1]) : NaN;
}

function renderDisplayThumbnails(surface, events) {
  const candidates = events.filter((event) => matchesReconstructableDisplayEvent(event));
  const step = Math.max(1, Math.ceil(candidates.length / 10));
  for (const event of candidates.filter((_, index) => index % step === 0).slice(0, 10)) {
    const image = el("img", { class: "display-thumb", alt: "", src: displayFrameUrl(event.hostMonotonicNs) });
    image.style.left = `calc(${scale(event.hostMonotonicNs, state.focusStartNs, state.focusEndNs) * 100}% - 1.6rem)`;
    image.addEventListener("error", () => image.remove());
    surface.append(image);
  }
}

function matchesReconstructableDisplayEvent(event) {
  return event.source === "display" && ["display.scanout", "display.update"].includes(event.kind) && event.artifactRefs?.length;
}

function attachTimeSurface(surface, events, startNs, endNs) {
  surface.addEventListener("pointerdown", (pointerEvent) => {
    const rect = surface.getBoundingClientRect();
    surface.setPointerCapture(pointerEvent.pointerId);
    state.drag = { surface, pointerId: pointerEvent.pointerId, startX: pointerEvent.clientX, rect, startNs, endNs };
  });
  surface.addEventListener("pointerup", (pointerEvent) => finishTimeGesture(pointerEvent, events));
  surface.addEventListener("pointermove", (pointerEvent) => showTrackTooltip(pointerEvent, surface, events, startNs, endNs));
  surface.addEventListener("pointerleave", hideTrackTooltip);
  surface.addEventListener("click", (clickEvent) => {
    if (clickEvent.detail !== 0) return;
    const selected = Number.isFinite(state.cursorNs) && state.cursorNs >= startNs && state.cursorNs <= endNs
      ? state.cursorNs
      : Math.round((startNs + endNs) / 2);
    const event = nearestEvent(events, selected);
    selectTime(selected, event);
  });
}

function finishTimeGesture(pointerEvent, events) {
  if (!state.drag || state.drag.pointerId !== pointerEvent.pointerId) return;
  const { rect, startX, startNs, endNs } = state.drag;
  const first = timeAtClientX(startX, rect, startNs, endNs);
  const last = timeAtClientX(pointerEvent.clientX, rect, startNs, endNs);
  const distance = Math.abs(pointerEvent.clientX - startX);
  state.drag = null;
  if (distance >= 8) {
    setFocus(Math.min(first, last), Math.max(first, last));
    return;
  }
  const selected = timeAtClientX(pointerEvent.clientX, rect, startNs, endNs);
  const nearest = nearestEvent(events, selected);
  const tolerance = (endNs - startNs) * 0.012;
  selectTime(selected, nearest && Math.abs(nearest.hostMonotonicNs - selected) <= tolerance ? nearest : null);
}

function timeAtClientX(clientX, rect, startNs, endNs) {
  return Math.round(startNs + Math.max(0, Math.min(1, (clientX - rect.left) / rect.width)) * (endNs - startNs));
}

function showTrackTooltip(pointerEvent, surface, events, startNs, endNs) {
  if (state.drag || !events.length) return;
  const time = timeAtClientX(pointerEvent.clientX, surface.getBoundingClientRect(), startNs, endNs);
  const event = nearestEvent(events, time);
  if (!event) return;
  let tooltip = $(".track-tooltip");
  if (!tooltip) { tooltip = el("div", { class: "track-tooltip", role: "tooltip" }); document.body.append(tooltip); }
  tooltip.textContent = `${relativeTime(event.hostMonotonicNs)} · ${event.source}\n${event.kind}\n${eventSummary(event)}`;
  tooltip.style.left = `${Math.min(innerWidth - 420, pointerEvent.clientX + 12)}px`;
  tooltip.style.top = `${Math.min(innerHeight - 110, pointerEvent.clientY + 12)}px`;
}
function hideTrackTooltip() { $(".track-tooltip")?.remove(); }

function positionCursor(node, timeNs, startNs, endNs) {
  if (!Number.isFinite(timeNs) || timeNs < startNs || timeNs > endNs) { node.hidden = true; return; }
  node.hidden = false;
  node.style.left = `${scale(timeNs, startNs, endNs) * 100}%`;
}

function renderTable() {
  const root = $("#event-rows");
  root.replaceChildren();
  for (const event of state.events) {
    const row = el("tr", { tabindex: "0", "data-event-id": event.id, "aria-selected": event.id === state.selectedId ? "true" : "false" });
    row.append(
      el("td", { class: "mono", text: `${relativeTime(event.hostMonotonicNs)} · ${formatWallTime(event)}` }),
      el("td", { text: event.source }),
      el("td", { class: "mono", text: event.kind, title: event.kind }),
      el("td", { text: eventSummary(event), title: eventSummary(event) }),
      el("td", { class: "mono", text: formatCount(event.artifactRefs?.length) }),
      el("td", { text: provenanceLabel(event.provenance) }),
    );
    row.addEventListener("click", () => selectEvent(event.id));
    row.addEventListener("keydown", (keyboardEvent) => { if (keyboardEvent.key === "Enter" || keyboardEvent.key === " ") { keyboardEvent.preventDefault(); selectEvent(event.id); } });
    root.append(row);
  }
}

function selectEvent(id, announce = true) {
  const event = state.events.find((candidate) => candidate.id === id);
  if (!event) return;
  state.selectedId = event.id;
  state.cursorNs = event.hostMonotonicNs;
  renderWorkspace();
  if (announce) $("#announcer").textContent = `Selected ${event.kind} from ${event.source} at ${formatWallTime(event)}`;
}

function selectTime(timeNs, event = null) {
  state.cursorNs = Math.max(state.run.startNs, Math.min(state.run.endNs, Math.round(timeNs)));
  state.selectedId = event?.id || null;
  renderWorkspace();
  $("#announcer").textContent = event
    ? `Selected ${event.kind} at ${formatWallTime(event)}`
    : `Selected machine time ${relativeTime(state.cursorNs)} with no exact event`;
}

function navigateWholeRunTime(timeNs) {
  const selected = Math.max(state.run.startNs, Math.min(state.run.endNs, Math.round(timeNs)));
  const [start, end] = focusRangeAtTime(
    selected,
    state.focusStartNs,
    state.focusEndNs,
    state.run.startNs,
    state.run.endNs,
  );
  if (start === state.focusStartNs && end === state.focusEndNs) {
    const event = nearestEvent(state.events, selected);
    const tolerance = focusSpan() * 0.012;
    selectTime(selected, event && Math.abs(event.hostMonotonicNs - selected) <= tolerance ? event : null);
    return;
  }
  state.cursorNs = selected;
  state.selectedId = null;
  state.events = [];
  state.relations = [];
  setFocus(start, end);
  renderWorkspace();
  $("#announcer").textContent = `Loading synchronized evidence around ${relativeTime(selected)}`;
}

function contextEvents() {
  if (!Number.isFinite(state.cursorNs)) return [];
  const radius = Math.min(10_000_000_000, Math.max(2_000_000_000, focusSpan() * 0.025));
  return state.events.filter((event) => Math.abs(event.hostMonotonicNs - state.cursorNs) <= radius);
}

function renderEvidenceCanvas() {
  const nearby = contextEvents();
  const logs = nearby.filter((event) => trackForEvent(event) === "logs");
  const network = nearby.filter((event) => trackForEvent(event) === "network");
  const artifacts = [];
  const seen = new Set();
  for (const event of nearby) {
    for (const reference of event.artifactRefs || []) {
      if (seen.has(reference)) continue;
      seen.add(reference);
      artifacts.push({ reference, event });
    }
  }
  renderLogRegion(logs);
  renderNetworkRegion(network);
  renderArtifactRegion(artifacts);
  renderSelectionRegion();
  if (Number.isFinite(state.cursorNs)) loadDisplayAt(state.cursorNs);
}

function renderLogRegion(events) {
  $("#logs-count").textContent = formatCount(events.length);
  const root = $("#logs-content");
  if (!events.length) { root.replaceChildren(emptyRegion("No log evidence around this time")); return; }
  const list = el("ol", { class: "data-list" });
  for (const event of events.slice(0, 200)) {
    const row = el("li", { class: "data-row interactive-row", tabindex: "0", role: "button", "aria-current": event.id === state.selectedId ? "true" : "false" }, [
      el("time", { class: "mono", text: relativeTime(event.hostMonotonicNs) }),
      el("span", {}, [el("strong", { text: event.kind }), document.createTextNode(` · ${eventSummary(event)}`)]),
    ]);
    wireEvidenceRow(row, event);
    list.append(row);
  }
  root.replaceChildren(list);
}

function renderNetworkRegion(events) {
  $("#network-count").textContent = formatCount(events.length);
  const root = $("#network-content");
  if (!events.length) { root.replaceChildren(emptyRegion("No network evidence around this time")); return; }
  const list = el("ol", { class: "data-list" });
  for (const event of events.slice(0, 200)) {
    const payload = event.payload || {};
    const row = el("li", { class: "data-row network-row interactive-row", tabindex: "0", role: "button", "aria-current": event.id === state.selectedId ? "true" : "false" }, [
      el("time", { class: "mono", text: relativeTime(event.hostMonotonicNs) }),
      el("span", { class: "mono", text: String(payload.method || payload.type || event.kind.split(".").at(-1) || "—") }),
      el("span", { text: String(payload.url || payload.host || payload.path || eventSummary(event)), title: String(payload.url || payload.host || payload.path || eventSummary(event)) }),
      el("span", { class: "status mono", text: String(payload.status || payload.statusCode || "—") }),
    ]);
    wireEvidenceRow(row, event);
    list.append(row);
  }
  root.replaceChildren(list);
}

function wireEvidenceRow(row, event) {
  row.setAttribute("aria-label", `Select ${event.kind} at ${relativeTime(event.hostMonotonicNs)}`);
  wireSelectableEvidenceRow(row, event.id, selectEvent);
}

export function wireSelectableEvidenceRow(row, eventId, select) {
  row.addEventListener("click", () => select(eventId));
  row.addEventListener("keydown", (keyboardEvent) => {
    if (keyboardEvent.key !== "Enter" && keyboardEvent.key !== " ") return;
    keyboardEvent.preventDefault();
    select(eventId);
  });
}

function renderArtifactRegion(items) {
  $("#artifacts-count").textContent = formatCount(items.length);
  const root = $("#artifacts-content");
  const display = el("section", { class: "display-state compact", id: "selected-display" }, [el("p", { text: "Checking display evidence…" })]);
  if (!items.length) { root.replaceChildren(display, emptyRegion("No recorded artifacts around this time")); return; }
  const list = el("ol", { class: "data-list" });
  for (const { reference, event } of items.slice(0, 200)) {
    const digest = reference.replace(/^sha256:/, "");
    const preview = matchesReconstructableDisplayEvent(event)
      ? el("img", { class: "artifact-preview", src: displayFrameUrl(event.hostMonotonicNs), alt: "Reconstructed display thumbnail" })
      : el("span", { class: "artifact-preview track-symbol", "aria-hidden": "true", text: trackForEvent(event).slice(0, 1).toUpperCase() });
    if (preview.tagName === "IMG") preview.addEventListener("error", () => preview.replaceWith(el("span", { class: "artifact-preview track-symbol", text: "?" })));
    const link = el("a", { href: `/api/artifacts/${digest}`, target: "_blank", rel: "noopener", class: "mono", text: shortId(reference, 12), title: reference });
    list.append(el("li", { class: "data-row artifact-row" }, [preview, el("span", {}, [link, el("small", { text: `${event.source} · ${event.kind}` })]), el("time", { class: "mono", text: relativeTime(event.hostMonotonicNs) })]));
  }
  root.replaceChildren(display, list);
}

function emptyRegion(message) { return el("div", { class: "data-empty", text: message }); }

function renderSelectionRegion() {
  const root = $("#selection-content");
  if (!Number.isFinite(state.cursorNs)) {
    $("#selected-time").textContent = "—";
    $("#selection-status").textContent = "No time selected";
    root.replaceChildren(emptyRegion("Select any point in the trace to inspect synchronized evidence"));
    return;
  }
  const event = state.events.find((candidate) => candidate.id === state.selectedId) || null;
  $("#selected-time").textContent = relativeTime(state.cursorNs);
  $("#selection-status").textContent = event ? `${event.source} · ${event.kind} · ${shortId(event.id)}` : `${relativeTime(state.cursorNs)} · no exact event`;
  const sheet = el("div", { class: "selection-sheet" });
  if (event) {
    sheet.append(selectionBlock("Evidence", [
      ["Source", event.source], ["Kind", event.kind], ["Time", formatWallTime(event)], ["ID", event.id],
      ["Provenance", provenanceLabel(event.provenance)], ["Artifacts", formatCount(event.artifactRefs?.length)],
    ]));
    sheet.append(selectionBlock("Semantic summary", [["Summary", eventSummary(event)], ["Relationships", formatCount(state.relations.filter((edge) => edge.fromEventId === event.id || edge.toEventId === event.id).length)]]));
    sheet.append(el("pre", { class: "payload", text: JSON.stringify(event.payload || {}, null, 2) }));
  } else {
    sheet.append(selectionBlock("Selected machine time", [["Run time", relativeTime(state.cursorNs)], ["Exact event", "None"], ["Context", "Surrounding evidence remains visible in every region"]]));
  }
  root.replaceChildren(sheet);
}

function selectionBlock(title, values) {
  const dl = el("dl");
  for (const [term, description] of values) dl.append(el("dt", { text: term }), el("dd", { class: String(description).includes("-") ? "mono" : "", text: String(description), title: String(description) }));
  return el("section", { class: "selection-block" }, [el("h3", { text: title }), dl]);
}

async function loadDisplayAt(timeNs) {
  const token = ++state.frameToken;
  const root = $("#selected-display");
  try {
    const status = await api(`/api/frame-status?timeNs=${Math.round(timeNs)}`);
    if (token !== state.frameToken || !root) return;
    if (!status.available) {
      root.replaceChildren(el("p", { text: status.message }), el("small", { class: "mono", text: displayCoverageText(status) }));
      return;
    }
    const image = el("img", { alt: `Reconstructed display state at ${relativeTime(timeNs)}` });
    image.addEventListener("load", () => {
      if (token === state.frameToken) root.replaceChildren(image, el("small", { class: "mono", text: `Display state at selection · last update ${relativeTime(status.frameNs)}` }));
    });
    image.addEventListener("error", () => {
      if (token !== state.frameToken) return;
      root.replaceChildren(el("p", { text: "Display evidence exists, but the frame image could not be loaded." }));
      showProblem(Object.assign(new Error("Frame image request failed."), { problem: { code: "frame_image_failed", path: image.src } }), "Display request failed");
    });
    image.src = `/api/frame-at?timeNs=${Math.round(timeNs)}`;
  } catch (error) {
    if (token !== state.frameToken) return;
    root.replaceChildren(el("p", { text: "Display evidence could not be checked. Other evidence remains available." }));
    showProblem(error, "Display request failed");
  }
}

function displayCoverageText(status) {
  if (!status.captureStartNs) return status.state.replaceAll("_", " ");
  return `${status.state.replaceAll("_", " ")} · capture ${relativeTime(status.captureStartNs)} — ${relativeTime(status.captureEndNs)}`;
}

function setPerspective(value, shouldRender = true) {
  state.perspective = value;
  $("#trace-view-button")?.setAttribute("aria-pressed", value === "trace" ? "true" : "false");
  $("#table-view-button")?.setAttribute("aria-pressed", value === "table" ? "true" : "false");
  if (shouldRender && state.run) renderWorkspace();
}

function setFocus(startNs, endNs) {
  const minimum = state.run.startNs ?? 0;
  const maximum = state.run.endNs ?? minimum;
  if (maximum <= minimum) {
    state.focusStartNs = minimum;
    state.focusEndNs = maximum;
    state.cursorNs = null;
    state.selectedId = null;
    loadEvents();
    return;
  }
  state.focusStartNs = Math.max(minimum, Math.min(maximum, Math.round(startNs)));
  state.focusEndNs = Math.max(state.focusStartNs + 1, Math.min(maximum, Math.round(endNs)));
  if (state.cursorNs < state.focusStartNs || state.cursorNs > state.focusEndNs) {
    state.cursorNs = Math.round((state.focusStartNs + state.focusEndNs) / 2);
    state.selectedId = null;
  }
  loadEvents();
}

function zoom(factor) {
  const anchor = Number.isFinite(state.cursorNs) ? state.cursorNs : (state.focusStartNs + state.focusEndNs) / 2;
  const [start, end] = zoomedRange(state.focusStartNs, state.focusEndNs, anchor, factor, state.run.startNs, state.run.endNs);
  setFocus(start, end);
}

function resetRange() { setFocus(state.run.startNs ?? 0, state.run.endNs ?? state.run.startNs ?? 0); }

function clearFilters() {
  state.query = "";
  $("#query").value = "";
  state.activeSources = new Set(Object.keys(state.run.sourceCounts));
  state.activeProvenance = new Set(PROVENANCE);
  $$('[data-source-filter], [data-provenance-filter]').forEach((input) => { input.checked = true; });
  $("#filter-menu").open = false;
  loadEvents();
}

function toggleMaximize(regionName) {
  const region = $(`[data-region="${CSS.escape(regionName)}"]`);
  const opening = !region.classList.contains("maximized");
  $$(".evidence-region.maximized").forEach((item) => item.classList.remove("maximized"));
  region.classList.toggle("maximized", opening);
  const button = region.querySelector(".maximize");
  button.textContent = opening ? "↙" : "↗";
  button.setAttribute("aria-label", opening ? `Restore ${regionName}` : `Maximize ${regionName}`);
}

function showProblem(error, title) {
  const problem = error.problem || { code: "inspector_error", message: error.message };
  state.problem = { title, ...problem, message: problem.message || error.message, detail: problem.detail || error.message };
  $("#problem-title").textContent = title;
  $("#problem-message").textContent = state.problem.message;
  $("#problem-id").textContent = state.problem.requestId ? `request ${state.problem.requestId}` : state.problem.code;
  $("#problem-shelf").hidden = false;
}

function clearProblem() {
  state.problem = null;
  $("#problem-shelf").hidden = true;
}

function showFatal(error) {
  showProblem(error, "Inspector could not open this run");
  $("#trace-workspace").replaceChildren(el("section", { class: "empty-state" }, [
    el("h1", { text: "The run could not be opened" }),
    el("p", { text: error.message }),
  ]));
  $("#run-identity").textContent = "Unavailable";
}

function updateUrl() {
  if (!state.run) return;
  const url = new URL(location.href);
  url.searchParams.set("view", state.perspective);
  if (state.selectedId) url.searchParams.set("event", state.selectedId); else url.searchParams.delete("event");
  if (Number.isFinite(state.cursorNs)) url.searchParams.set("time", String(Math.round(state.cursorNs)));
  if (state.focusStartNs !== state.run.startNs) url.searchParams.set("start", String(Math.round(state.focusStartNs))); else url.searchParams.delete("start");
  if (state.focusEndNs !== state.run.endNs) url.searchParams.set("end", String(Math.round(state.focusEndNs))); else url.searchParams.delete("end");
  if (state.query) url.searchParams.set("query", state.query); else url.searchParams.delete("query");
  history.replaceState({}, "", url);
}

function bindControls() {
  $("#trace-view-button").addEventListener("click", () => setPerspective("trace"));
  $("#table-view-button").addEventListener("click", () => setPerspective("table"));
  $("#zoom-in").addEventListener("click", () => zoom(0.5));
  $("#zoom-out").addEventListener("click", () => zoom(2));
  $("#reset-range").addEventListener("click", resetRange);
  $("#clear-filters").addEventListener("click", clearFilters);
  $$('[data-clear-filters]').forEach((button) => button.addEventListener("click", clearFilters));
  $("#dismiss-problem").addEventListener("click", clearProblem);
  $("#copy-problem").addEventListener("click", async () => {
    if (!state.problem) return;
    await navigator.clipboard.writeText(JSON.stringify(state.problem, null, 2));
    $("#announcer").textContent = "Copied inspector diagnostics";
  });
  $$("[data-maximize]").forEach((button) => button.addEventListener("click", () => toggleMaximize(button.dataset.maximize)));
  let queryTimer;
  $("#query-form").addEventListener("submit", (event) => { event.preventDefault(); clearTimeout(queryTimer); state.query = $("#query").value.trim(); loadEvents(); });
  $("#query").addEventListener("input", () => { clearTimeout(queryTimer); queryTimer = setTimeout(() => { state.query = $("#query").value.trim(); loadEvents(); }, 280); });
  attachWholeRunNavigator();
  $("#trace-view").addEventListener("wheel", (wheelEvent) => {
    if (!(wheelEvent.ctrlKey || wheelEvent.metaKey || wheelEvent.shiftKey)) return;
    wheelEvent.preventDefault();
    if (wheelEvent.shiftKey && !(wheelEvent.ctrlKey || wheelEvent.metaKey)) {
      const delta = focusSpan() * Math.sign(wheelEvent.deltaY || wheelEvent.deltaX) * 0.12;
      const [start, end] = zoomedRange(state.focusStartNs + delta, state.focusEndNs + delta, (state.focusStartNs + state.focusEndNs) / 2 + delta, 1, state.run.startNs, state.run.endNs);
      setFocus(start, end);
    } else zoom(wheelEvent.deltaY > 0 ? 1.35 : 0.74);
  }, { passive: false });
  addEventListener("resize", debounce(() => { if (state.run) renderWorkspace(); }, 120));
  document.addEventListener("keydown", handleKeyboard);
}

function attachWholeRunNavigator() {
  const surface = $("#overview-surface");
  surface.addEventListener("pointerdown", (pointerEvent) => {
    const rect = surface.getBoundingClientRect();
    surface.setPointerCapture(pointerEvent.pointerId);
    state.drag = { surface, pointerId: pointerEvent.pointerId, startX: pointerEvent.clientX, rect, startNs: state.run.startNs, endNs: state.run.endNs };
  });
  surface.addEventListener("pointerup", (pointerEvent) => {
    if (!state.drag || state.drag.pointerId !== pointerEvent.pointerId) return;
    const { rect, startX, startNs, endNs } = state.drag;
    const first = timeAtClientX(startX, rect, startNs, endNs);
    const last = timeAtClientX(pointerEvent.clientX, rect, startNs, endNs);
    const distance = Math.abs(pointerEvent.clientX - startX);
    state.drag = null;
    if (distance >= 8) setFocus(Math.min(first, last), Math.max(first, last));
    else navigateWholeRunTime(last);
  });
  surface.addEventListener("click", (clickEvent) => {
    if (clickEvent.detail !== 0) return;
    const selected = Number.isFinite(state.cursorNs) ? state.cursorNs : Math.round((state.run.startNs + state.run.endNs) / 2);
    navigateWholeRunTime(selected);
  });
}

function handleKeyboard(event) {
  const typing = ["INPUT", "TEXTAREA"].includes(document.activeElement?.tagName);
  if (event.key === "/" && !typing) { event.preventDefault(); $("#query").focus(); return; }
  if (typing) return;
  if (event.key.toLowerCase() === "t") { setPerspective("trace"); return; }
  if (event.key.toLowerCase() === "a") { setPerspective("table"); return; }
  if (event.key === "+" || event.key === "=") { zoom(0.5); return; }
  if (event.key === "-") { zoom(2); return; }
  if (event.key === "Escape") {
    const maximized = $(".evidence-region.maximized");
    if (maximized) toggleMaximize(maximized.dataset.region);
    return;
  }
  if (["ArrowLeft", "ArrowRight"].includes(event.key) && state.events.length) {
    event.preventDefault();
    const current = state.events.findIndex((item) => item.id === state.selectedId);
    const next = event.key === "ArrowRight" ? Math.min(state.events.length - 1, current + 1) : Math.max(0, current < 0 ? 0 : current - 1);
    selectEvent(state.events[next].id);
  }
}

function debounce(callback, wait) {
  let timer;
  return (...arguments_) => { clearTimeout(timer); timer = setTimeout(() => callback(...arguments_), wait); };
}

if (typeof document !== "undefined") initialize();
