// Navigation benchmark for the web app's tile maps (#635): pans and zooms a real map in Chrome and
// reports frame intervals, memory and console errors, the yardstick for the tile layer's steps.
//
//   cd apps/rustgis-web && trunk build --release
//   python3 -m http.server -d ../../dist/web 8780 &
//   node bench/navigate.mjs [--url http://127.0.0.1:8780/] [--case liberty|imagery|all]
//                           [--gpu] [--webgl] [--headless] [--json out.json]
//                           [--dpr 2] [--throttle 4] [--profile]
//
// Needs Playwright (`npm i -g playwright`, then `playwright install chromium`) and the network:
// the cases draw OpenFreeMap's Liberty style over Manhattan and Esri World Imagery over Paris.
// Each case loads the app, waits for the first tiles, then runs the same scripted drags and wheel
// zooms. `--dpr` sets the device pixel ratio (a laptop's 2), `--throttle` slows the CPU that many
// times (Chrome's CPU throttling) to stand in for a slower machine. Frame intervals are measured
// with requestAnimationFrame in the page, so they include every stall of the main thread (fetch
// callbacks, decoding, drawing, uploads).

import { createRequire } from "node:module";
import { execSync } from "node:child_process";
import { writeFileSync } from "node:fs";

const require = createRequire(import.meta.url);
function loadPlaywright() {
  try {
    return require("playwright");
  } catch {
    // A global install isn't on Node's module path for an ES module.
    const root = execSync("npm root -g").toString().trim();
    return require(`${root}/playwright`);
  }
}
const { chromium } = loadPlaywright();

const args = process.argv.slice(2);
const opt = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  if (i < 0) return fallback;
  // Given without its value (`--dpr --gpu`): an error rather than the default.
  if (!args[i + 1] || args[i + 1].startsWith("--")) throw new Error(`--${name} needs a value`);
  return args[i + 1];
};
const flag = (name) => args.includes(`--${name}`);

const base = opt("url", "http://127.0.0.1:8780/");
// A device pixel ratio and a CPU slowdown that the run really uses, or an error.
const number = (name, fallback, min) => {
  const v = Number(opt(name, String(fallback)));
  if (!Number.isFinite(v) || v < min) throw new Error(`--${name} takes a number of at least ${min}`);
  return v;
};
const dpr = number("dpr", 1, 0.5);
const throttle = number("throttle", 1, 1);
const which = opt("case", "all");
const CASES = {
  liberty: { query: "?style=https://tiles.openfreemap.org/styles/liberty", hash: "#14/40.7549/-73.9840" },
  imagery: {
    query: "?tiles=https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}",
    hash: "#13/48.8566/2.3522",
  },
};
const names = which === "all" ? Object.keys(CASES) : [which];

// Watches the page: animation-frame intervals, the wasm memory's size, long tasks.
const PROBE = () => {
  const s = (window.__bench = { frames: [], longTasks: 0, longest: 0, recording: false, memory: null });
  addEventListener("TrunkApplicationStarted", (e) => (s.memory = e.detail?.wasm?.memory ?? null));
  let last = performance.now();
  const tick = (t) => {
    if (s.recording) s.frames.push(t - last);
    last = t;
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
  try {
    new PerformanceObserver((list) => {
      for (const e of list.getEntries()) {
        if (!s.recording) continue;
        s.longTasks += 1;
        s.longest = Math.max(s.longest, e.duration);
      }
    }).observe({ type: "longtask", buffered: false });
  } catch {}
};

const pct = (xs, p) => {
  if (!xs.length) return 0;
  const sorted = [...xs].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length))];
};
const mb = (n) => Math.round((n / (1 << 20)) * 10) / 10;

// The browser of the case under way, closed if the case fails, and whether its page crashed.
let opened = null;
let pageCrashed = false;

async function run(name) {
  const c = CASES[name];
  pageCrashed = false;
  // Headless Chrome on Linux draws WebGL2 on the real GPU through ANGLE's EGL backend; its WebGPU
  // is SwiftShader's software adapter there, so headless runs want `--webgl`.
  const launchArgs = ["--enable-unsafe-webgpu", "--use-gl=angle", "--use-angle=gl-egl", "--ignore-gpu-blocklist", "--enable-gpu"];
  const browser = await chromium.launch({ headless: flag("headless"), args: launchArgs });
  opened = browser;
  const page = await browser.newPage({ viewport: { width: 1480, height: 900 }, deviceScaleFactor: dpr });
  const console = { error: 0, warning: 0, samples: [] };
  page.on("console", (m) => {
    const t = m.type();
    if (t !== "error" && t !== "warning") return;
    console[t] += 1;
    if (console.samples.length < 8) console.samples.push(`${t}: ${m.text().slice(0, 200)}`);
  });
  // Tile requests made while navigating: each one is fetched and decoded on the main thread.
  const requests = { all: 0, failed: 0, recording: false };
  page.on("request", (r) => {
    if (requests.recording && /^https?:/.test(r.url()) && !r.url().startsWith(base)) requests.all += 1;
  });
  page.on("requestfailed", (r) => {
    if (requests.recording && !r.url().startsWith(base)) requests.failed += 1;
  });
  let crashed = false;
  page.on("crash", () => {
    crashed = true;
    pageCrashed = true;
  });
  page.on("pageerror", (e) => {
    console.error += 1;
    if (console.samples.length < 8) console.samples.push(`pageerror: ${String(e).slice(0, 200)}`);
  });
  await page.addInitScript(PROBE);
  if (flag("gpu")) await page.addInitScript(() => localStorage.setItem("rustgis.ui.map.gpu", "1"));
  const query = c.query + (flag("webgl") ? "&webgl" : "");
  await page.goto(base + query + c.hash);
  await page.waitForFunction(() => !document.getElementById("rustgis_loading"), null, { timeout: 120000 });
  // The first tiles land and the map settles.
  await page.waitForTimeout(8000);
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Performance.enable");
  if (throttle > 1) await cdp.send("Emulation.setCPUThrottlingRate", { rate: throttle });
  const heap = async () => {
    const m = await cdp.send("Performance.getMetrics");
    const js = m.metrics.find((x) => x.name === "JSHeapUsedSize")?.value ?? 0;
    const wasm = await page.evaluate(() => window.__bench.memory?.buffer?.byteLength ?? 0);
    return { js, wasm };
  };
  const before = await heap();

  // The map's middle, clear of the ribbon above and the panes beside it.
  const cx = 740;
  const cy = 520;
  await page.mouse.move(cx, cy);
  if (flag("profile")) {
    await cdp.send("Profiler.enable");
    await cdp.send("Profiler.setSamplingInterval", { interval: 200 });
    await cdp.send("Profiler.start");
  }
  await page.evaluate(() => (window.__bench.recording = true));
  requests.recording = true;
  const started = Date.now();
  // Drags: a pan out and back in each direction, ~60 mouse moves a drag.
  for (const [dx, dy] of [[420, 0], [-420, 0], [0, 260], [0, -260], [300, 200], [-300, -200]]) {
    await page.mouse.down();
    for (let i = 1; i <= 60; i++) await page.mouse.move(cx + (dx * i) / 60, cy + (dy * i) / 60);
    await page.mouse.up();
    await page.mouse.move(cx, cy);
    await page.waitForTimeout(400);
  }
  // Wheel zooms: out three levels, in four, out one, a notch at a time.
  for (const [notches, dir] of [[18, 1], [24, -1], [6, 1]]) {
    for (let i = 0; i < notches; i++) {
      await page.mouse.wheel(0, dir * 100);
      await page.waitForTimeout(40);
    }
    await page.waitForTimeout(800);
  }
  // A fast pan across many tiles while zoomed: fetches pile up behind it.
  await page.mouse.down();
  for (let i = 1; i <= 120; i++) await page.mouse.move(cx - (900 * i) / 120, cy - (300 * (i % 40)) / 40);
  await page.mouse.up();
  await page.waitForTimeout(3000);
  const seconds = (Date.now() - started) / 1000;
  const s = await page.evaluate(() => {
    const b = window.__bench;
    b.recording = false;
    return { frames: b.frames, longTasks: b.longTasks, longest: b.longest };
  });
  requests.recording = false;
  // `--profile`: the functions the main thread spent most of its own time in while navigating
  // (a build with names: trunk's `data-keep-debug`, no wasm-opt).
  let profile = null;
  if (flag("profile")) {
    const { profile: p } = await cdp.send("Profiler.stop");
    const self = new Map();
    const byId = new Map(p.nodes.map((n) => [n.id, n]));
    const dt = new Map();
    p.samples.forEach((id, i) => dt.set(id, (dt.get(id) ?? 0) + (p.timeDeltas[i] ?? 0)));
    for (const [id, us] of dt) {
      const f = byId.get(id)?.callFrame;
      const name = f ? f.functionName || `(${f.url.split("/").pop()})` : "?";
      // The main thread waiting (the script's pauses) isn't work.
      if (name === "(idle)") continue;
      self.set(name, (self.get(name) ?? 0) + us);
    }
    const total = [...self.values()].reduce((a, b) => a + b, 0);
    profile = [...self.entries()]
      .sort((a, b) => b[1] - a[1])
      .slice(0, 30)
      .map(([n, us]) => `${((100 * us) / total).toFixed(1)}% ${n.slice(0, 140)}`);
  }
  const shot = opt("shot", null);
  if (shot) await page.screenshot({ path: shot.replace(/(\.png)?$/, `-${name}.png`) });
  const after = await heap();
  await browser.close();
  const f = s.frames;
  return {
    case: name,
    gpu: flag("gpu"),
    dpr,
    throttle,
    webgl: flag("webgl"),
    crashed,
    seconds: Math.round(seconds * 10) / 10,
    frames: f.length,
    p50: Math.round(pct(f, 50) * 10) / 10,
    p95: Math.round(pct(f, 95) * 10) / 10,
    p99: Math.round(pct(f, 99) * 10) / 10,
    max: Math.round(Math.max(0, ...f) * 10) / 10,
    over50: f.filter((x) => x > 50).length,
    over100: f.filter((x) => x > 100).length,
    longTasks: s.longTasks,
    longestTask: Math.round(s.longest),
    requests: requests.all,
    requestsFailed: requests.failed,
    jsHeapMB: [mb(before.js), mb(after.js)],
    wasmMB: [mb(before.wasm), mb(after.wasm)],
    console: { errors: console.error, warnings: console.warning, samples: console.samples },
    profile,
  };
}

const results = [];
for (const name of names) {
  // A crashed page rejects the next Playwright call: reported as a crash, not lost.
  const r = await run(name).catch(async (e) => {
    await opened?.close().catch(() => {});
    return { case: name, crashed: pageCrashed, failed: true, error: String(e).slice(0, 300) };
  });
  results.push(r);
  process.stdout.write(JSON.stringify(r, null, 2) + "\n");
}
const out = opt("json", null);
if (out) writeFileSync(out, JSON.stringify(results, null, 2));
