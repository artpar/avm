# WebUI whole-machine evidence workspace

Implementation status (2026-08-15): the direction C shell, shared-time lanes,
consolidated evidence canvas, Events perspective, structured problems, and
selected-time display semantics are implemented and dogfooded through AVM. The
remaining acceptance work is tracked by active items UI-006, UI-007, UI-009,
and UI-011 in `backlog.md`.

Status: confirmed design brief, 2026-08-15  
Canonical work tracker: [`backlog.md`](../backlog.md)  
Confirmed direction: [visual probe C](assets/webui-direction-c.png)

The probe fixes the topology, hierarchy, and density target. Its generated
labels, example data, dimensions, and incidental styling are not a pixel-level
specification.

## Feature summary

AVM is a consolidated inspector for everything collected from a virtual machine
while software is being developed: CPU, disk, network, logs, browser, input,
display, audio, VM lifecycle, artifacts, and future collectors. The interface
must make the machine's behavior across time understandable with minimal effort,
without forcing the developer to choose one evidence type before seeing another.

The causal Trace remains primary. Events remains a switchable chronological
ledger. Both are views over one focus range, one selected time, one set of
filters, and one evidence record.

## Primary user action

Move through recorded time and see what every collected source was doing around
the same moment. Selecting a point, event, or artifact updates the entire
workspace atomically; it never sends the user into a separate inspection flow.

When an event-specific URL is opened, AVM preserves that exact selection.
Otherwise it selects the latest high-signal failure, or the latest collected
event when the run has no failure, while keeping the whole-run range visible.

## Design direction

- **Color strategy:** restrained, strictly grayscale. State uses value,
  typography, shape, pattern, and labels rather than hue.
- **Scene:** a software developer at a normal desk in bright ambient light is
  examining a long, information-rich run and wants the tool to disappear into
  the investigation.
- **Anchors:** Chrome DevTools Performance and Perfetto for multiscale tracks;
  Playwright Trace Viewer for point-in-time evidence synchronization; rr for a
  stable recorded-time mental model.
- **Character:** dense, quiet, precise, familiar, and semantic. No ornamental
  dashboard treatment, screen-player framing, or card grid.

## Scope

- **Fidelity:** production-ready.
- **Breadth:** the complete WebUI surface, not one panel.
- **Interactivity:** shipped-quality navigation, filtering, resizing,
  maximization, keyboard use, deep links, and failure handling.
- **Time intent:** polish and dogfood through AVM until the acceptance gate
  passes on real application-development runs.

The WebUI remains read-only with respect to guest and evidence state. Read-only
is enforced by architecture, not advertised as persistent page chrome.

## Information model

```text
run
└── whole-run time domain
    ├── focus range
    │   ├── shared-time evidence lanes
    │   │   ├── CPU
    │   │   ├── disk
    │   │   ├── network
    │   │   ├── logs
    │   │   ├── browser
    │   │   ├── input
    │   │   ├── display
    │   │   ├── audio
    │   │   ├── VM lifecycle
    │   │   └── future collectors
    │   └── selected time / event
    │       ├── nearby logs
    │       ├── nearby network activity
    │       ├── artifacts across all sources
    │       └── structured semantics and provenance
    └── coverage, gaps, truncation, and collection bounds per source
```

An event is a semantic landmark within time, not the container for all evidence.
An artifact is evidence associated with time and source, not content hidden
inside an event tab.

## Layout strategy

### Command strip

One compact row contains run identity, semantic search, Trace/Events perspective,
current focus range, reset, and overflow/help. It contains no policy badges,
large metrics, or guest controls.

### Shared-time trace

The upper workspace uses a single horizontal ruler. Every visible source owns a
lane aligned to it:

- CPU uses aggregated utilization with min/max disclosure at coarse scales;
- disk separates reads, writes, latency, and file-operation landmarks;
- network combines throughput with request spans and status semantics;
- logs show scale-aware density and severity/type shapes, expanding to records;
- browser shows navigation/document spans and browser evidence landmarks;
- input shows compact keyboard, pointer, scroll, and action marks;
- display uses sparse thumbnails/updates without becoming a player;
- audio uses waveform/envelope and explicit collected/silent/absent semantics;
- VM shows running, reset, paused, snapshot, and lifecycle intervals.

Lane headers carry source name, visible/total count, coverage status, and one
consistent disclosure control. Lanes may be collapsed or resized. Reordering is
deferred until real dogfood proves it valuable; a stable default order reduces
configuration work.

### Consolidated evidence canvas

The lower workspace contains four persistent, resizable regions:

1. nearby logs;
2. network requests/waterfall;
3. artifacts, grouped by type/source rather than privileging frames; and
4. selected-time semantics, relationships, raw fields, and provenance.

All four are populated for the current focus/selection and remain visible by
default. A region may temporarily maximize inline and restore with the same
control or Escape. Maximization never changes the selected time.

Display frames and audio clips live inside the artifact region and their source
lanes. When no display evidence exists, the preview is black and says `No display
evidence at this time`. AVM never silently substitutes a nearby frame. A recorded
black frame has artifact/timestamp metadata and is therefore not confused with
absence.

### Events perspective

Events is a virtualized, sortable ledger with time, source, semantic evidence,
relationships, provenance, and artifact counts. It retains the same focus range
and selection as Trace. Switching perspectives is instant and reversible.

### Responsive structure

Desktop is the primary environment. At narrower widths, source labels compress
before evidence disappears. The lower canvas becomes a two-by-two arrangement,
then a vertical sequence. It never becomes mutually exclusive evidence tabs.

## Interaction model

- Click any lane mark, ledger row, log, request, or artifact to select its time
  and synchronize every region.
- Click empty track space to select that exact time without inventing an event.
- Drag across the ruler or lanes to create a focus range.
- Wheel/pinch over the ruler zooms around the pointer; horizontal pan preserves
  scale. Reset returns to the whole run.
- Hover provides lightweight exact values and cross-source alignment; it never
  hides the committed selection.
- Search filters/highlights matching evidence across lanes and the ledger while
  making omitted counts explicit.
- Source controls change visibility, not collection history. Turning off every
  source produces an honest empty trace.
- Left/right moves to the previous/next meaningful evidence time; plus/minus
  zooms; Escape exits maximization; all controls have visible equivalents.
- Focus range, cursor, perspective, filters, and selected evidence serialize to
  the URL for stable sharing and reload.

Live runs append evidence without stealing the cursor. A `Follow latest` mode is
explicit and disengages as soon as the user navigates backward.

## Key states

### Loading

Preserve the final layout with lightweight skeleton rows/tracks. Load summary,
coverage index, coarse track aggregates, then fine evidence for the focus range.
No global spinner blocks already available evidence.

### Empty run

Show the time domain and collector roster, explaining that no evidence has been
collected yet. Do not fabricate zero-valued tracks.

### Source absent or outside coverage

An absent lane is labeled `Not collected`; a selected time outside its coverage
is labeled `No evidence at this time`. Zero CPU, zero traffic, silence, and a
recorded black frame are genuine values and must remain distinct from absence.

### Collection gap or truncation

Gaps occupy their real span in the lane with a neutral pattern and explicit
reason. Result limits and coarse aggregation are stated with visible counts.

### Artifact corrupt or missing

Keep its metadata visible, mark verification state, and expose the artifact ID
and request diagnostics. One broken artifact must not blank unrelated evidence.

### Request failure

The affected region retains its last valid or partial content and shows a compact
problem row with error code, HTTP status, request ID, and copyable detail.
Structured server logs correlate the same request ID. The product never reports
only `Query failed`.

### No exact selected event

The cursor still represents a valid time. Selected-time semantics say `No event
at this exact time` and show surrounding evidence; AVM does not snap silently.

## Content and semantics

Every source adapter supplies:

- coverage spans and collection bounds;
- aggregation at multiple time scales;
- stable identifiers and timestamps;
- semantic summary plus structured raw fields;
- artifact and relationship references;
- provenance and integrity state; and
- source-specific zero, absence, gap, corruption, and truncation semantics.

Counts always state whether they mean whole run, focus range, filter result, or
visible rows. Units remain visible. Absolute, monotonic, and run-relative time
are available without repeating all three in every cell.

## Architecture implications

- Build a multiresolution read index for track aggregates and source coverage;
  do not fetch all raw events merely to paint an overview.
- Give all endpoints one structured problem schema and request ID.
- Separate selected time from selected event in application state and routes.
- Resolve cross-source evidence for a focus range and cursor atomically enough to
  prevent stale panels after rapid navigation.
- Virtualize the ledger and high-volume lower tables; declare completeness and
  aggregation level in responses.
- Keep new collectors pluggable through a typed lane/summary contract rather
  than hardcoding display/browser assumptions into the shell.

## Delivery sequence

1. **Foundation:** UI-007 structured diagnostics; selected-time state; coverage
   and aggregation contracts; fixtures for every source and absence state.
2. **Trace shell:** UI-008 command strip, shared ruler, synchronized lanes, and
   consolidated lower canvas using existing evidence.
3. **Time engine:** UI-009 zoom, pan, focus range, cursor, URL persistence,
   coarse-to-fine loading, and live-run follow semantics.
4. **Source depth:** UI-006 honest coverage/gaps plus source-specific encodings
   for CPU, disk, network, logs, browser, input, display, audio, and VM.
5. **Ledger and cleanup:** Events parity, UI-010 hierarchy/copy cleanup,
   resizing/maximization, keyboard and responsive behavior.
6. **AVM-on-AVM gate:** UI-011 dogfood on the audited 506-event run, fresh
   application-development runs, absent-source fixtures, and 20,000+ events;
   inspect AVM's own collected evidence throughout development.

Implementation slices must remain vertically usable: each slice includes its
API, trace/lower-region presentation, states, tests, and dogfood evidence. Do not
build all backend aggregation first and postpone the usable interface.

## Acceptance gate

- On first load, the user can see which sources were collected, their activity
  and gaps across the run, and evidence around the selected time without opening
  an evidence tab.
- One navigation gesture updates every lane and lower region consistently.
- CPU, disk, network, logs, browser, input, display, audio, VM, and artifacts are
  all present when collected; no modality dominates the default layout.
- Absence is never rendered as zero, silence, or recorded black output.
- Every permanent region answers a concrete whole-machine question or provides
  navigation; there is no inert density panel or policy-status chrome.
- A developer can move from whole-run overview to a one-second focus and back
  without losing selection, filters, or context.
- Failed requests are diagnosable from the UI and correlated server logs.
- The interface remains usable with keyboard only, reduced motion, 200% zoom,
  and representative high-volume runs.
- AVM is used to develop and validate every implementation slice, with findings
  recorded in `backlog.md`.

## Implementation references

During implementation, apply the frontend design guidance for layout, product
interaction, hardening, adaptation, typography, and performance. Validate the
result in the live browser at each vertical slice; the confirmed raster probe is
a direction test, not a substitute for working-product judgment.
