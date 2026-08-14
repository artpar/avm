import { chromium } from "playwright-core";
import { mkdir } from "node:fs/promises";
import { randomUUID } from "node:crypto";
import { fileURLToPath } from "node:url";
import { join, resolve } from "node:path";

export function parseArguments(argv) {
  const options = { endpoint: null, trace: null, artifactsDir: null, durationMs: 0 };
  for (let index = 0; index < argv.length; index += 2) {
    const name = argv[index];
    const value = argv[index + 1];
    if (value === undefined) throw new Error(`missing value for ${name}`);
    if (name === "--endpoint") options.endpoint = value;
    else if (name === "--trace") options.trace = value;
    else if (name === "--artifacts-dir") options.artifactsDir = value;
    else if (name === "--duration-ms") options.durationMs = Number(value);
    else throw new Error(`unknown argument ${name}`);
  }
  if (!options.endpoint) throw new Error("--endpoint is required");
  if (!options.trace) throw new Error("--trace is required");
  if (!options.artifactsDir) throw new Error("--artifacts-dir is required");
  if (!Number.isSafeInteger(options.durationMs) || options.durationMs < 0) {
    throw new Error("--duration-ms must be a non-negative integer");
  }
  return options;
}

export function unverifiedWindowEstimate(metrics) {
  const horizontalBorder = Math.max(0, metrics.outerWidth - metrics.innerWidth) / 2;
  const verticalChrome = Math.max(0, metrics.outerHeight - metrics.innerHeight);
  return {
    verified: false,
    authority: "browser_window_metrics_only",
    deviceScaleFactor: metrics.devicePixelRatio,
    viewport: {
      width: metrics.innerWidth,
      height: metrics.innerHeight,
      offsetLeft: metrics.visualViewport?.offsetLeft ?? 0,
      offsetTop: metrics.visualViewport?.offsetTop ?? 0,
      scale: metrics.visualViewport?.scale ?? 1,
    },
    displayContentOrigin: {
      x: metrics.screenX + horizontalBorder,
      y: metrics.screenY + verticalChrome,
    },
    formula:
      "display = displayContentOrigin + (viewport - visualViewport.offset) * deviceScaleFactor / visualViewport.scale",
  };
}

function emit(source, kind, payload, artifacts = []) {
  process.stdout.write(
    `${JSON.stringify({
      source,
      kind,
      sourceTimestamp: {
        processMonotonicNs: process.hrtime.bigint().toString(),
        wallClockTime: new Date().toISOString(),
      },
      payload,
      artifacts,
    })}\n`,
  );
}

async function pageState(page) {
  return page.evaluate(() => {
    const focused = document.activeElement;
    const focusedRect = focused?.getBoundingClientRect?.();
    return {
      url: location.href,
      title: document.title,
      visibilityState: document.visibilityState,
      hasFocus: document.hasFocus(),
      focusedElement: focused
        ? {
            tagName: focused.tagName,
            id: focused.id || null,
            name: focused.getAttribute("name"),
            role: focused.getAttribute("role"),
            accessibleName:
              focused.getAttribute("aria-label") || focused.textContent?.trim() || null,
            value: "value" in focused ? focused.value : null,
            rect: focusedRect
              ? {
                  x: focusedRect.x,
                  y: focusedRect.y,
                  width: focusedRect.width,
                  height: focusedRect.height,
                }
              : null,
          }
        : null,
      windowMetrics: {
        screenX: window.screenX,
        screenY: window.screenY,
        outerWidth: window.outerWidth,
        outerHeight: window.outerHeight,
        innerWidth: window.innerWidth,
        innerHeight: window.innerHeight,
        devicePixelRatio: window.devicePixelRatio,
        visualViewport: window.visualViewport
          ? {
              offsetLeft: window.visualViewport.offsetLeft,
              offsetTop: window.visualViewport.offsetTop,
              width: window.visualViewport.width,
              height: window.visualViewport.height,
              scale: window.visualViewport.scale,
            }
          : null,
      },
    };
  });
}

async function captureSemanticSnapshot(page, cdp, reason, artifactsDir) {
  const screenshotPath = join(artifactsDir, `viewport-${randomUUID()}.png`);
  const [state, _screenshot, dom, accessibility, performance] = await Promise.all([
    pageState(page),
    page.screenshot({ path: screenshotPath }),
    cdp.send("DOMSnapshot.captureSnapshot", {
      computedStyles: ["display", "visibility", "opacity", "pointer-events"],
      includeDOMRects: true,
      includePaintOrder: true,
    }),
    cdp.send("Accessibility.getFullAXTree"),
    cdp.send("Performance.getMetrics"),
  ]);
  emit(
    "browser",
    "browser.page.snapshot",
    {
      reason,
      state,
      unverifiedWindowEstimate: unverifiedWindowEstimate(state.windowMetrics),
      dom,
      accessibility,
    },
    [{ path: screenshotPath, role: "browser.viewport", mimeType: "image/png" }],
  );
  emit("performance", "browser.performance.metrics", {
    reason,
    url: state.url,
    metrics: performance.metrics,
  });
}

async function observePage(page, artifactsDir) {
  const cdp = await page.context().newCDPSession(page);
  await Promise.all([
    cdp.send("Accessibility.enable"),
    cdp.send("DOM.enable"),
    cdp.send("Network.enable"),
    cdp.send("Page.enable"),
    cdp.send("Performance.enable"),
    cdp.send("Runtime.enable"),
  ]);

  let snapshotTimer = null;
  const scheduleSnapshot = (reason) => {
    clearTimeout(snapshotTimer);
    snapshotTimer = setTimeout(() => {
      captureSemanticSnapshot(page, cdp, reason, artifactsDir).catch((error) =>
        emit("browser", "browser.snapshot.failed", { reason, error: String(error) }),
      );
    }, 100);
  };

  page.on("framenavigated", (frame) => {
    if (frame === page.mainFrame()) {
      emit("browser", "browser.navigation", { url: frame.url() });
      scheduleSnapshot("navigation");
    }
  });
  page.on("console", (message) =>
    emit("console", "browser.console.message", {
      type: message.type(),
      text: message.text(),
      location: message.location(),
    }),
  );
  page.on("pageerror", (error) =>
    emit("console", "browser.javascript.exception", {
      name: error.name,
      message: error.message,
      stack: error.stack,
    }),
  );
  page.on("request", (request) =>
    emit("network", "browser.network.request", {
      url: request.url(),
      method: request.method(),
      resourceType: request.resourceType(),
      isNavigationRequest: request.isNavigationRequest(),
      postData: request.postData(),
    }),
  );
  page.on("response", async (response) => {
    let contentType = null;
    let headersError = null;
    try {
      contentType = (await response.allHeaders())["content-type"] ?? null;
    } catch (error) {
      headersError = String(error);
    }
    emit("network", "browser.network.response", {
      url: response.url(),
      status: response.status(),
      statusText: response.statusText(),
      fromServiceWorker: response.fromServiceWorker(),
      contentType,
      headersError,
    });
  });
  page.on("websocket", (socket) => {
    emit("network", "browser.websocket.opened", { url: socket.url() });
    socket.on("framesent", (event) =>
      emit("network", "browser.websocket.frame_sent", {
        url: socket.url(),
        payload: event.payload,
      }),
    );
    socket.on("framereceived", (event) =>
      emit("network", "browser.websocket.frame_received", {
        url: socket.url(),
        payload: event.payload,
      }),
    );
    socket.on("close", () =>
      emit("network", "browser.websocket.closed", { url: socket.url() }),
    );
  });

  await page.exposeBinding("__avmDomMutation", (_source, mutation) => {
    emit("browser", "browser.dom.mutation", mutation);
    scheduleSnapshot("dom_mutation");
  });
  const installMutationObserver = () => {
    if (globalThis.__avmMutationObserverInstalled) return;
    globalThis.__avmMutationObserverInstalled = true;
    const observer = new MutationObserver((records) => {
      globalThis.__avmDomMutation({
        count: records.length,
        records: records.slice(0, 50).map((record) => ({
          type: record.type,
          target: record.target?.nodeName ?? null,
          attributeName: record.attributeName,
          addedNodes: record.addedNodes?.length ?? 0,
          removedNodes: record.removedNodes?.length ?? 0,
        })),
      });
    });
    observer.observe(document, {
      subtree: true,
      childList: true,
      attributes: true,
      characterData: true,
    });
  };
  await page.addInitScript(installMutationObserver);
  await page.evaluate(installMutationObserver);
  await captureSemanticSnapshot(page, cdp, "observer_attached", artifactsDir);
}

export async function runObserver(options) {
  await mkdir(options.artifactsDir, { recursive: true });
  const browser = await chromium.connectOverCDP(options.endpoint, {
    timeout: 30_000,
  });
  const contexts = browser.contexts();
  if (contexts.length === 0) throw new Error("CDP browser has no default context");
  const context = contexts[0];
  await context.tracing.start({ screenshots: true, snapshots: true, sources: false });
  const observed = new WeakSet();
  const attach = async (page) => {
    if (observed.has(page)) return;
    observed.add(page);
    await observePage(page, options.artifactsDir);
  };
  context.on("page", (page) => attach(page).catch((error) => emit("browser", "browser.attach.failed", { error: String(error) })));
  await Promise.all(context.pages().map(attach));
  emit("browser", "browser.observer.started", {
    endpoint: options.endpoint,
    pageCount: context.pages().length,
  });

  await new Promise((resolvePromise) => {
    let timer = null;
    const finish = () => {
      clearTimeout(timer);
      resolvePromise();
    };
    process.once("SIGINT", finish);
    process.once("SIGTERM", finish);
    browser.once("disconnected", finish);
    if (options.durationMs > 0) timer = setTimeout(finish, options.durationMs);
  });
  await context.tracing.stop({ path: options.trace });
  emit("browser", "browser.observer.completed", { trace: options.trace });
  await browser.close();
}

if (resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  runObserver(parseArguments(process.argv.slice(2))).catch((error) => {
    process.stderr.write(`${error.stack ?? error}\n`);
    process.exitCode = 1;
  });
}
