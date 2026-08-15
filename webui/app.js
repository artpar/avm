const state = {
  run: null,
  overview: null,
  events: [],
  relations: [],
  activeSources: new Set(),
  activeProvenance: new Set(["observed", "derived", "model_interpreted", "agent_claim"]),
  selectedId: null,
  perspective: "trace",
  query: "",
  abort: null,
};

const $ = (selector) => document.querySelector(selector);
const $$ = (selector) => [...document.querySelectorAll(selector)];
const sourceOrder = ["display", "input", "browser", "accessibility", "runtime", "agent", "transport", "policy", "artifacts", "vm", "cursor", "audio", "performance", "temporal", "vlm", "codex"];
const sourceGlyphs = { display: "▣", input: "↥", browser: "◎", accessibility: "A", runtime: "⌁", agent: "◇", transport: "↔", policy: "⊞", artifacts: "□", vm: "V", cursor: "+", audio: "∿", performance: "∆", temporal: "T", vlm: "M", codex: "C" };

export function sourceFilterValue(activeSources, totalSources) {
  if (activeSources.size === totalSources) return null;
  return [...activeSources].sort().join(",");
}

export function replaceRelationPaths(container, paths) {
  container.replaceChildren(...paths);
}

export function reloadFrameWhenSelected(frameTab, loader) {
  if (frameTab.getAttribute("aria-selected") !== "true") return false;
  loader();
  return true;
}

async function api(path, options = {}) {
  const response = await fetch(path, { ...options, headers: { Accept: "application/json", ...options.headers } });
  if (!response.ok) {
    const problem = await response.json().catch(() => ({ message: response.statusText }));
    throw new Error(problem.message || `Request failed (${response.status})`);
  }
  return response.json();
}

function formatCount(value) { return new Intl.NumberFormat().format(value ?? 0); }
function formatDuration(ns) {
  if (!Number.isFinite(ns)) return "—";
  if (Math.abs(ns) < 1_000) return `${ns} ns`;
  if (Math.abs(ns) < 1_000_000) return `${(ns / 1_000).toFixed(1)} μs`;
  if (Math.abs(ns) < 1_000_000_000) return `${(ns / 1_000_000).toFixed(1)} ms`;
  return `${(ns / 1_000_000_000).toFixed(3)} s`;
}
function formatTime(event) {
  const date = new Date(event.wallClockTime);
  if (Number.isNaN(date.valueOf())) return event.wallClockTime;
  return `${date.toISOString().slice(11, 23)} UTC`;
}
function shortId(value, size = 8) { return value ? `${value.slice(0, size)}…${value.slice(-4)}` : "—"; }
function humanSource(source) { return source ? source[0].toUpperCase() + source.slice(1) : "Unknown"; }
function eventSummary(event) {
  const payload = event.payload || {};
  const preferred = [payload.summary, payload.message, payload.url, payload.text, payload.status, payload.error, payload.key, payload.button, payload.frameSha256];
  const value = preferred.find((item) => item !== undefined && item !== null && String(item).trim());
  if (value !== undefined) return String(value);
  const entries = Object.entries(payload).slice(0, 2).map(([key, item]) => `${key}=${typeof item === "object" ? "…" : item}`);
  return entries.join(" · ") || "No payload fields";
}
function provenanceLabel(value) {
  return ({ observed: "Raw", derived: "Derived", model_interpreted: "Model", agent_claim: "Claim" })[value] || value;
}

function el(tag, attributes = {}, children = []) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attributes)) {
    if (key === "class") node.className = value;
    else if (key === "text") node.textContent = value;
    else if (key.startsWith("data-")) node.setAttribute(key, value);
    else if (value !== null && value !== undefined) node.setAttribute(key, value);
  }
  for (const child of Array.isArray(children) ? children : [children]) if (child) node.append(child);
  return node;
}

async function initialize() {
  bindControls();
  try {
    const [run, overview] = await Promise.all([api("/api/run"), api("/api/overview?buckets=240")]);
    state.run = run;
    state.overview = overview;
    state.activeSources = new Set(Object.keys(run.sourceCounts));
    renderRun();
    renderOverview();
    await loadEvents();
    restoreDeepLink();
  } catch (error) {
    showFatal(error);
  }
}

function renderRun() {
  const run = state.run;
  $("#run-identity").textContent = `Run ${shortId(run.runId)} · ${formatCount(run.eventCount)} events`;
  const facts = $("#run-facts");
  facts.replaceChildren(
    el("dt", { text: "Events" }), el("dd", { text: formatCount(run.eventCount) }),
    el("dt", { text: "Artifacts" }), el("dd", { text: formatCount(run.artifactCount) }),
    el("dt", { text: "Sources" }), el("dd", { text: formatCount(Object.keys(run.sourceCounts).length) }),
    el("dt", { text: "Duration" }), el("dd", { text: formatDuration((run.endNs || 0) - (run.startNs || 0)) }),
    el("dt", { text: "Viewport" }), el("dd", { text: `${run.width}×${run.height}` }),
  );
  const list = $("#source-list");
  list.replaceChildren();
  Object.entries(run.sourceCounts)
    .sort(([a], [b]) => (sourceOrder.indexOf(a) < 0 ? 99 : sourceOrder.indexOf(a)) - (sourceOrder.indexOf(b) < 0 ? 99 : sourceOrder.indexOf(b)) || a.localeCompare(b))
    .forEach(([source, count]) => {
      const button = el("button", { type: "button", class: "source-row", "data-source": source, "aria-pressed": "true", title: `Toggle ${source} events` }, [
        el("span", { class: "source-symbol", "aria-hidden": "true", text: sourceGlyphs[source] || "·" }),
        el("span", { text: humanSource(source) }),
        el("span", { class: "source-count mono", text: formatCount(count) }),
      ]);
      button.addEventListener("click", () => toggleSource(source, button));
      list.append(button);
    });
}

function renderOverview() {
  const overview = state.overview;
  const density = $("#density");
  density.replaceChildren();
  if (!overview.buckets.length) return;
  const max = Math.max(...overview.buckets.map((bucket) => bucket.count), 1);
  const nonzero = overview.buckets.filter((bucket) => bucket.count > 0).map((bucket) => bucket.count);
  const typical = nonzero.sort((a, b) => a - b)[Math.floor(nonzero.length / 2)] || 1;
  overview.buckets.forEach((bucket) => {
    const isGap = bucket.count === 0 && typical > 0;
    const bar = el("span", {
      class: `density-bar${isGap ? " gap" : ""}`,
      title: isGap ? `Collection gap · ${formatDuration(bucket.endNs - bucket.startNs)}` : `${formatCount(bucket.count)} events`,
      "aria-hidden": "true",
    });
    if (!isGap) bar.style.height = `${Math.max(5, Math.sqrt(bucket.count / max) * 100)}%`;
    density.append(bar);
  });
  const duration = overview.endNs - overview.startNs;
  $("#overview-range").textContent = `${formatDuration(duration)} · ${formatCount(overview.buckets.length)} buckets`;
  $("#overview-axis").replaceChildren(
    el("span", { text: formatTimeFromNs(overview.startNs) }),
    el("span", { text: `+${formatDuration(duration / 2)}` }),
    el("span", { text: `+${formatDuration(duration)}` }),
  );
}

function formatTimeFromNs(ns) { return `${(ns / 1_000_000_000).toFixed(3)} s`; }

async function loadEvents() {
  state.abort?.abort();
  state.abort = new AbortController();
  $("#result-state").textContent = "Loading events…";
  if (state.activeSources.size === 0) {
    state.events = [];
    state.relations = [];
    state.selectedId = null;
    renderEvents();
    $("#result-state").textContent = "0 events";
    return;
  }
  const params = new URLSearchParams({ limit: "20000" });
  if (state.query) params.set("query", state.query);
  const sourceFilter = sourceFilterValue(state.activeSources, Object.keys(state.run.sourceCounts).length);
  if (sourceFilter !== null) params.set("source", sourceFilter);
  if (state.activeProvenance.size !== 4) params.set("provenance", [...state.activeProvenance].join(","));
  try {
    const result = await api(`/api/events?${params}`, { signal: state.abort.signal });
    state.events = result.events;
    state.relations = result.relations;
    if (!state.events.some((event) => event.id === state.selectedId)) state.selectedId = null;
    renderEvents();
    $("#result-state").textContent = result.truncated ? `${formatCount(result.events.length)} of ${formatCount(result.totalBeforeLimit)}` : `${formatCount(result.events.length)} events`;
  } catch (error) {
    if (error.name !== "AbortError") showInlineError(error);
  }
}

function renderEvents() {
  const empty = state.events.length === 0;
  if (!state.selectedId) {
    setEvidenceOpen(false);
    $("#selection-status").textContent = "No event selected";
  }
  $("#empty-state").hidden = !empty;
  $("#trace-view").hidden = empty || state.perspective !== "trace";
  $("#table-view").hidden = empty || state.perspective !== "table";
  renderTrace();
  renderTable();
}

function renderTrace() {
  const lanes = $("#lanes");
  lanes.replaceChildren();
  $("#relations").replaceChildren();
  if (!state.events.length) return;
  const start = state.events[0].hostMonotonicNs;
  const end = state.events[state.events.length - 1].hostMonotonicNs;
  const span = Math.max(end - start, 1);
  renderRuler(span);
  const grouped = Map.groupBy ? Map.groupBy(state.events, (event) => event.source) : groupBy(state.events, (event) => event.source);
  const ordered = [...grouped.keys()].sort((a, b) => (sourceOrder.indexOf(a) < 0 ? 99 : sourceOrder.indexOf(a)) - (sourceOrder.indexOf(b) < 0 ? 99 : sourceOrder.indexOf(b)) || a.localeCompare(b));
  for (const source of ordered) {
    const lane = el("div", { class: "lane", "data-lane": source });
    lane.append(el("div", { class: "lane-label" }, [
      el("span", { class: "source-symbol", "aria-hidden": "true", text: sourceGlyphs[source] || "·" }),
      el("span", { text: humanSource(source) }),
    ]));
    for (const event of grouped.get(source)) {
      const left = 7 + ((event.hostMonotonicNs - start) / span) * 93;
      const mark = el("button", {
        type: "button", class: "event-mark", "data-event-id": event.id, "data-provenance": event.provenance,
        "aria-current": event.id === state.selectedId ? "true" : "false",
        "aria-label": `${formatTime(event)}, ${event.source}, ${event.kind}, ${eventSummary(event)}`,
        title: `${event.kind}\n${eventSummary(event)}\n${formatTime(event)}`,
        text: event.kind,
      });
      mark.style.left = `${left}%`;
      mark.addEventListener("click", () => selectEvent(event.id));
      lane.append(mark);
    }
    lanes.append(lane);
  }
  requestAnimationFrame(renderRelations);
}

function groupBy(items, key) {
  const map = new Map();
  for (const item of items) { const value = key(item); if (!map.has(value)) map.set(value, []); map.get(value).push(item); }
  return map;
}

function renderRuler(span) {
  const ruler = $("#time-ruler");
  ruler.replaceChildren();
  for (let index = 0; index <= 5; index++) {
    const tick = el("span", { class: "tick", text: `+${formatDuration(span * index / 5)}` });
    tick.style.left = `${index * 20}%`;
    ruler.append(tick);
  }
}

function renderRelations() {
  const svg = $("#relations");
  const root = $("#trace-view").getBoundingClientRect();
  const selectedRelations = state.selectedId ? state.relations.filter((edge) => edge.fromEventId === state.selectedId || edge.toEventId === state.selectedId) : state.relations.slice(0, 120);
  const paths = [];
  for (const relation of selectedRelations.slice(0, 200)) {
    const from = $(`[data-event-id="${CSS.escape(relation.fromEventId)}"]`);
    const to = $(`[data-event-id="${CSS.escape(relation.toEventId)}"]`);
    if (!from || !to || from.tagName === "TR" || to.tagName === "TR") continue;
    const a = from.getBoundingClientRect(); const b = to.getBoundingClientRect();
    const x1 = a.left - root.left - 112 + a.width / 2; const y1 = a.top - root.top - 32 + a.height / 2;
    const x2 = b.left - root.left - 112 + b.width / 2; const y2 = b.top - root.top - 32 + b.height / 2;
    const mid = (x1 + x2) / 2;
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    path.setAttribute("d", `M ${x1} ${y1} C ${mid} ${y1}, ${mid} ${y2}, ${x2} ${y2}`);
    path.setAttribute("class", `relation ${relation.basis}`);
    paths.push(path);
  }
  replaceRelationPaths(svg, paths);
}

function renderTable() {
  const tbody = $("#event-rows");
  tbody.replaceChildren();
  let previous = null;
  for (const event of state.events) {
    const row = el("tr", { tabindex: "0", "data-event-id": event.id, "aria-selected": event.id === state.selectedId ? "true" : "false" });
    row.append(
      el("td", { class: "mono", text: formatTime(event) }),
      el("td", { class: "mono", text: previous ? `+${formatDuration(event.hostMonotonicNs - previous.hostMonotonicNs)}` : "—" }),
      el("td", { text: event.source }),
      el("td", { class: "mono", text: event.kind }),
      el("td", { text: eventSummary(event), title: eventSummary(event) }),
      el("td", { text: provenanceLabel(event.provenance) }),
    );
    row.addEventListener("click", () => selectEvent(event.id));
    row.addEventListener("keydown", (keyboardEvent) => { if (keyboardEvent.key === "Enter") selectEvent(event.id); });
    tbody.append(row);
    previous = event;
  }
}

function selectEvent(id, announce = true) {
  state.selectedId = id;
  const event = state.events.find((candidate) => candidate.id === id);
  if (!event) return;
  setEvidenceOpen(true);
  for (const node of $$('[data-event-id]')) {
    if (node.matches("tr")) node.setAttribute("aria-selected", node.dataset.eventId === id ? "true" : "false");
    else node.setAttribute("aria-current", node.dataset.eventId === id ? "true" : "false");
  }
  renderEvidence(event);
  if (state.perspective === "trace") requestAnimationFrame(renderRelations);
  const url = new URL(location.href); url.searchParams.set("event", id); url.searchParams.set("view", state.perspective); history.replaceState({}, "", url);
  if (announce) $("#announcer").textContent = `Selected ${event.kind} from ${event.source} at ${formatTime(event)}`;
}

function renderEvidence(event) {
  $("#evidence-title").textContent = `${event.kind} · ${event.source}`;
  $("#selected-time").textContent = formatTime(event);
  $("#selection-status").textContent = `${event.source} · ${event.kind} · ${shortId(event.id)}`;
  const payloadEntries = Object.entries(event.payload || {});
  $("#evidence-summary").replaceChildren(el("div", { class: "summary-grid" }, [
    summaryGroup("Event", [["ID", event.id], ["Source", event.source], ["Kind", event.kind], ["Sequence", event.sourceSequence ?? "—"]]),
    summaryGroup("Timing", [["Wall clock", formatTime(event)], ["Host monotonic", `${event.hostMonotonicNs} ns`], ["From run start", formatDuration(event.hostMonotonicNs - state.run.startNs)]]),
    summaryGroup("Record", [["Provenance", provenanceLabel(event.provenance)], ["Artifacts", formatCount(event.artifactRefs.length)], ["Fingerprint", shortId(event.repositoryFingerprint, 14)], ["Payload fields", formatCount(payloadEntries.length)]]),
    summaryGroup("Semantic preview", [["Summary", eventSummary(event)], ...payloadEntries.slice(0, 3).map(([key, value]) => [key, typeof value === "object" ? JSON.stringify(value) : String(value)])]),
  ]));
  $("#evidence-payload").replaceChildren(el("pre", { class: "payload", text: JSON.stringify(event.payload, null, 2) }));
  renderArtifacts(event);
  renderProvenance(event);
  renderFrame(event);
}

function summaryGroup(title, values) {
  const dl = el("dl");
  for (const [term, description] of values) dl.append(el("dt", { text: term }), el("dd", { class: String(description).includes("sha256:") || String(description).includes("-") ? "mono" : "", text: String(description), title: String(description) }));
  return el("section", { class: "summary-group" }, [el("h3", { text: title }), dl]);
}

function renderArtifacts(event) {
  const panel = $("#evidence-artifacts");
  if (!event.artifactRefs.length) {
    panel.replaceChildren(el("div", { class: "frame-state" }, [el("p", { text: "No artifact was recorded for this event." })]));
    return;
  }
  const list = el("ul", { class: "artifact-list" });
  event.artifactRefs.forEach((reference, index) => {
    const digest = reference.replace(/^sha256:/, "");
    const link = el("a", { href: `/api/artifacts/${digest}`, target: "_blank", rel: "noopener", class: "mono", text: reference, title: reference });
    list.append(el("li", {}, [link, el("span", { text: index === 0 ? "Primary" : `Artifact ${index + 1}` })]));
  });
  panel.replaceChildren(list);
}

function renderProvenance(event) {
  const related = state.relations.filter((edge) => edge.fromEventId === event.id || edge.toEventId === event.id);
  const list = el("div", { class: "summary-grid" }, [
    summaryGroup("Classification", [["Type", provenanceLabel(event.provenance)], ["Meaning", provenanceMeaning(event.provenance)], ["Repository", event.repositoryFingerprint || "Not recorded"]]),
    summaryGroup("Relationships", [["Recorded/shared bases", related.length], ...related.slice(0, 6).map((edge) => [edge.basis.replaceAll("_", " "), edge.fromEventId === event.id ? `to ${shortId(edge.toEventId)}` : `from ${shortId(edge.fromEventId)}`])]),
    summaryGroup("Source clock", [["Timestamp", event.sourceTimestamp ? JSON.stringify(event.sourceTimestamp) : "No separate source clock"], ["Sequence", event.sourceSequence ?? "Not recorded"]]),
  ]);
  $("#evidence-provenance").replaceChildren(list);
}

function provenanceMeaning(value) {
  return ({ observed: "Recorded directly by an AVM-owned sensor or boundary.", derived: "Produced deterministically from recorded observations.", model_interpreted: "Interpretation produced by an identified model.", agent_claim: "Assertion made by an agent; inspect supporting evidence." })[value] || "Unknown provenance; inspect the raw envelope.";
}

function renderFrame(event) {
  const panel = $("#evidence-frame");
  panel.replaceChildren(el("div", { class: "frame-state" }, [el("p", { text: "Open Frame to reconstruct the display at this event." })]));
  panel.dataset.eventId = event.id;
  panel.dataset.loaded = "false";
  reloadFrameWhenSelected($("#tab-frame"), loadFramePanel);
}

async function loadFramePanel() {
  const panel = $("#evidence-frame");
  if (!panel.dataset.eventId || panel.dataset.loaded === "true") return;
  const eventId = panel.dataset.eventId;
  panel.dataset.loaded = "true";
  panel.replaceChildren(el("div", { class: "frame-state" }, [el("p", { text: "Reconstructing framebuffer without modifying evidence…" })]));
  const image = el("img", { alt: "Historical guest framebuffer at the selected event" });
  image.addEventListener("load", () => {
    if (panel.dataset.eventId === eventId) panel.replaceChildren(el("div", { class: "frame-state" }, [image]));
  });
  image.addEventListener("error", () => {
    if (panel.dataset.eventId === eventId) panel.replaceChildren(el("div", { class: "frame-state" }, [el("p", { text: "No reconstructable framebuffer exists at this point in the timeline." })]));
  });
  image.src = `/api/frames/${eventId}`;
}

function setPerspective(value) {
  state.perspective = value;
  $("#trace-view-button").setAttribute("aria-pressed", value === "trace" ? "true" : "false");
  $("#table-view-button").setAttribute("aria-pressed", value === "table" ? "true" : "false");
  renderEvents();
  const url = new URL(location.href); url.searchParams.set("view", value); history.replaceState({}, "", url);
}

function setTab(tab) {
  const tabs = $$('.evidence-tabs [role="tab"]');
  tabs.forEach((button) => { const active = button.id === `tab-${tab}`; button.setAttribute("aria-selected", active ? "true" : "false"); button.tabIndex = active ? 0 : -1; });
  $$('.evidence-body [role="tabpanel"]').forEach((panel) => { panel.hidden = panel.id !== `evidence-${tab}`; });
  if (tab === "frame") loadFramePanel();
}

function setEvidenceOpen(open) {
  const panel = $("#evidence");
  panel.hidden = !open;
  $(".main-stage").classList.toggle("evidence-collapsed", !open);
  if (!open && panel.contains(document.activeElement)) {
    $(state.perspective === "trace" ? "#trace-view-button" : "#table-view-button").focus();
  }
}

function toggleSource(source, button) {
  if (state.activeSources.has(source)) state.activeSources.delete(source); else state.activeSources.add(source);
  button.setAttribute("aria-pressed", state.activeSources.has(source) ? "true" : "false");
  loadEvents();
}

function clearFilters() {
  state.query = ""; $("#query").value = "";
  state.activeSources = new Set(Object.keys(state.run.sourceCounts));
  state.activeProvenance = new Set(["observed", "derived", "model_interpreted", "agent_claim"]);
  $$('.source-row, [data-provenance]').forEach((button) => button.setAttribute("aria-pressed", "true"));
  loadEvents();
}

function bindControls() {
  $("#trace-view-button").addEventListener("click", () => setPerspective("trace"));
  $("#table-view-button").addEventListener("click", () => setPerspective("table"));
  $("#clear-sources").addEventListener("click", () => { state.activeSources = new Set(Object.keys(state.run.sourceCounts)); $$('.source-row').forEach((button) => button.setAttribute("aria-pressed", "true")); loadEvents(); });
  $("#clear-filters").addEventListener("click", clearFilters);
  $$("[data-provenance]").forEach((button) => button.addEventListener("click", () => { const value = button.dataset.provenance; if (state.activeProvenance.has(value)) state.activeProvenance.delete(value); else state.activeProvenance.add(value); button.setAttribute("aria-pressed", state.activeProvenance.has(value) ? "true" : "false"); loadEvents(); }));
  let queryTimer;
  $("#query-form").addEventListener("submit", (event) => { event.preventDefault(); clearTimeout(queryTimer); state.query = $("#query").value.trim(); loadEvents(); });
  $("#query").addEventListener("input", () => { clearTimeout(queryTimer); queryTimer = setTimeout(() => { state.query = $("#query").value.trim(); loadEvents(); }, 280); });
  $$('.evidence-tabs [role="tab"]').forEach((button) => button.addEventListener("click", () => setTab(button.id.replace("tab-", ""))));
  addEventListener("resize", () => { if (state.perspective === "trace") requestAnimationFrame(renderRelations); });
  document.addEventListener("keydown", handleKeyboard);
}

function handleKeyboard(event) {
  const typing = ["INPUT", "TEXTAREA"].includes(document.activeElement?.tagName);
  if (event.key === "/" && !typing) { event.preventDefault(); $("#query").focus(); return; }
  if (typing) return;
  if (event.key.toLowerCase() === "t") { setPerspective("trace"); return; }
  if (event.key.toLowerCase() === "a") { setPerspective("table"); return; }
  if (event.key.toLowerCase() === "p") { setEvidenceOpen($("#evidence").hidden); return; }
  if (["ArrowLeft", "ArrowRight"].includes(event.key) && state.events.length) {
    event.preventDefault();
    const current = state.events.findIndex((item) => item.id === state.selectedId);
    const next = event.key === "ArrowRight" ? Math.min(state.events.length - 1, current + 1) : Math.max(0, current < 0 ? 0 : current - 1);
    selectEvent(state.events[next].id);
    $(`[data-event-id="${CSS.escape(state.events[next].id)}"]`)?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }
}

function restoreDeepLink() {
  const params = new URLSearchParams(location.search);
  if (params.get("view") === "table") setPerspective("table");
  const id = params.get("event");
  if (id && state.events.some((event) => event.id === id)) selectEvent(id, false);
  else if (state.events.length) selectEvent(state.events[Math.min(1, state.events.length - 1)].id, false);
}

function showInlineError(error) {
  $("#result-state").textContent = "Query failed";
  $("#empty-state").hidden = false;
  $("#empty-state h2").textContent = "Timeline query failed";
  $("#empty-state p").textContent = error.message;
}

function showFatal(error) {
  $("#workspace").replaceChildren(el("section", { class: "empty-state" }, [el("h1", { text: "Inspector could not open this run" }), el("p", { text: error.message })]));
  $("#run-identity").textContent = "Unavailable";
}

if (typeof document !== "undefined") initialize();
