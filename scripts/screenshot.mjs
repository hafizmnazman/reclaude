// Captures screenshots of the running app (pick + preview stages) via CDP.
// Run: node scripts/screenshot.mjs
import { spawn } from "node:child_process";
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const exe = join(here, "..", "src-tauri", "target", "release", "Reclaude.exe");
const PORT = 9224;

// Overridable for staging README shots: RECLAUDE_SHOT_FIXTURE (folder to
// create & inspect), RECLAUDE_SHOT_NEWNAME (name typed into the input).
const fixture =
  process.env.RECLAUDE_SHOT_FIXTURE ||
  join(tmpdir(), `reclaude-shot-${process.pid}`, "My Test Project");
const newName = process.env.RECLAUDE_SHOT_NEWNAME || "Renamed Project";
const ownsParent = !process.env.RECLAUDE_SHOT_FIXTURE;
mkdirSync(fixture, { recursive: true });

const child = spawn(exe, [], {
  env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}` },
  stdio: "ignore",
});

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function findPage() {
  for (let i = 0; i < 40; i++) {
    try {
      const pages = await (await fetch(`http://127.0.0.1:${PORT}/json`)).json();
      const page = pages.find((p) => p.type === "page" && /tauri\.localhost/.test(p.url));
      if (page) return page;
    } catch { }
    await sleep(250);
  }
  throw new Error("debugger endpoint never came up");
}

try {
  const page = await findPage();
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });
  let id = 0;
  const pending = new Map();
  ws.onmessage = (ev) => {
    const msg = JSON.parse(ev.data);
    if (msg.id && pending.has(msg.id)) { pending.get(msg.id)(msg); pending.delete(msg.id); }
  };
  const send = (method, params) =>
    new Promise((res) => { const m = ++id; pending.set(m, res); ws.send(JSON.stringify({ id: m, method, params })); });

  const shot = async (name) => {
    const r = await send("Page.captureScreenshot", { format: "png" });
    writeFileSync(join(here, name), Buffer.from(r.result.data, "base64"));
    console.log("saved", name);
  };

  await sleep(800);
  await shot("shot-1-pick.png");

  const fixtureJs = fixture.replaceAll("\\", "\\\\");
  await send("Runtime.evaluate", { expression: `pickFolder("${fixtureJs}")`, awaitPromise: true });
  await sleep(400);
  await send("Runtime.evaluate", {
    expression: `const i = document.getElementById("new-name"); i.value = ${JSON.stringify(newName)}; i.dispatchEvent(new Event("input"));`,
  });
  await sleep(900);
  await shot("shot-2-preview.png");
} catch (e) {
  console.error(e.message || e);
  process.exitCode = 1;
} finally {
  try { child.kill(); } catch { }
  rmSync(ownsParent ? dirname(fixture) : fixture, { recursive: true, force: true });
}
