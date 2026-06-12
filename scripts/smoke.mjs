// End-to-end smoke test: launches the built Reclaude.exe with WebView2 remote
// debugging, then drives the real app over CDP — checks the UI booted and
// exercises the IPC commands (inspect_project, preview_rename, last_manifest).
// Run: node scripts/smoke.mjs
import { spawn } from "node:child_process";
import { mkdirSync, rmSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const exe = join(here, "..", "src-tauri", "target", "release", "Reclaude.exe");
const PORT = 9223;

// fixture folder the app will inspect
const fixture = join(tmpdir(), `reclaude-smoke-${process.pid}`, "My Test Project");
mkdirSync(fixture, { recursive: true });

const child = spawn(exe, [], {
  env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}` },
  stdio: "ignore",
  detached: false,
});

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function findPage() {
  for (let i = 0; i < 40; i++) {
    try {
      const pages = await (await fetch(`http://127.0.0.1:${PORT}/json`)).json();
      const page = pages.find((p) => p.type === "page" && /tauri\.localhost/.test(p.url));
      if (page) return page;
    } catch { /* not up yet */ }
    await sleep(250);
  }
  throw new Error("debugger endpoint never came up");
}

let exitCode = 1;
try {
  const page = await findPage();
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });

  let id = 0;
  const pending = new Map();
  ws.onmessage = (ev) => {
    const msg = JSON.parse(ev.data);
    if (msg.id && pending.has(msg.id)) {
      pending.get(msg.id)(msg);
      pending.delete(msg.id);
    }
  };
  const send = (method, params) =>
    new Promise((res) => {
      const mid = ++id;
      pending.set(mid, res);
      ws.send(JSON.stringify({ id: mid, method, params }));
    });

  // wait until the app's DOM is actually built
  for (let i = 0; i < 40; i++) {
    const probe = await send("Runtime.evaluate", {
      expression: `!!document.getElementById("stage-pick")`,
      returnByValue: true,
    });
    if (probe.result.result.value === true) break;
    await sleep(250);
  }

  const fixtureJs = fixture.replaceAll("\\", "\\\\");
  const expr = `(async () => {
    const out = {};
    out.tauriInjected = !!window.__TAURI__;
    out.domBuilt = !!document.getElementById("browse-btn");
    out.pickStageVisible = !document.getElementById("stage-pick").hidden;
    const inv = window.__TAURI__.core.invoke;
    out.manifest = await inv("last_manifest");
    out.inspect = await inv("inspect_project", { path: "${fixtureJs}" });
    out.preview = await inv("preview_rename", { path: "${fixtureJs}", newName: "Renamed Project", deepFix: true });
    out.badName = await inv("preview_rename", { path: "${fixtureJs}", newName: "bad:name", deepFix: true });
    return JSON.stringify(out);
  })()`;

  const reply = await send("Runtime.evaluate", {
    expression: expr,
    awaitPromise: true,
    returnByValue: true,
  });
  if (reply.result.exceptionDetails) {
    throw new Error("page evaluate failed: " + JSON.stringify(reply.result.exceptionDetails));
  }
  const out = JSON.parse(reply.result.result.value);

  const assert = (cond, label) => {
    if (!cond) throw new Error("FAILED: " + label + "\n" + JSON.stringify(out, null, 2));
    console.log("ok -", label);
  };

  assert(out.tauriInjected, "__TAURI__ global injected");
  assert(out.domBuilt, "DOM built, browse button present");
  assert(out.pickStageVisible, "pick stage visible on boot");
  assert(out.inspect.name === "My Test Project", "inspect_project returns folder name");
  assert(/^c--/.test(out.inspect.encoded), "encoded name computed (lowercase drive)");
  assert(out.inspect.encodedExisting === null, "no history for fresh fixture");
  assert(out.preview.valid === true, "preview valid for good name");
  assert(out.preview.newPath.endsWith("Renamed Project"), "preview computes new path");
  assert(out.preview.historyFound === false, "preview reports no history");
  assert(out.badName.valid === false && /aren't allowed/.test(out.badName.error), "invalid name rejected with friendly error");

  console.log("\nSMOKE TEST PASSED");
  exitCode = 0;
} catch (e) {
  console.error(e.message || e);
} finally {
  try { child.kill(); } catch { /* already gone */ }
  rmSync(dirname(fixture), { recursive: true, force: true });
}
process.exit(exitCode);
