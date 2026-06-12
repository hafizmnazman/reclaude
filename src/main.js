"use strict";

const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;

const $ = (id) => document.getElementById(id);

const state = {
  inspection: null, // result of inspect_project for the picked folder
  preview: null,    // last valid preview
  result: null,     // last execute result
};

/* ---------------- theme ---------------- */

function applyTheme(theme) {
  document.documentElement.dataset.theme = theme;
  localStorage.setItem("reclaude-theme", theme);
  $("theme-icon-moon").hidden = theme !== "dark";
  $("theme-icon-sun").hidden = theme === "dark";
}

applyTheme(localStorage.getItem("reclaude-theme") || "dark");

$("theme-toggle").addEventListener("click", () => {
  const cur = document.documentElement.dataset.theme;
  applyTheme(cur === "dark" ? "light" : "dark");
});

/* ---------------- stages ---------------- */

function showStage(name) {
  for (const s of ["pick", "preview", "result"]) {
    const el = $("stage-" + s);
    el.hidden = s !== name;
    if (s === name) {
      // restart the fade-in animation
      el.style.animation = "none";
      void el.offsetWidth;
      el.style.animation = "";
    }
  }
}

/* ---------------- helpers ---------------- */

function escapeHtml(s) {
  return String(s)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    let ok = false;
    try { ok = document.execCommand("copy"); } catch { ok = false; }
    ta.remove();
    return ok;
  }
}

function flashButton(btn, text) {
  const orig = btn.textContent;
  btn.textContent = text;
  setTimeout(() => { btn.textContent = orig; }, 1400);
}

/* ---------------- stage 1: pick ---------------- */

async function pickFolder(path) {
  $("pick-error").hidden = true;
  let inspection;
  try {
    inspection = await invoke("inspect_project", { path });
  } catch (e) {
    $("pick-error").textContent = String(e);
    $("pick-error").hidden = false;
    return;
  }
  state.inspection = inspection;
  enterPreviewStage();
}

$("browse-btn").addEventListener("click", async () => {
  const picked = await invoke("plugin:dialog|open", {
    options: { directory: true, multiple: false, title: "Pick the project folder to rename" },
  });
  if (typeof picked === "string" && picked) {
    pickFolder(picked);
  }
});

listen("tauri://drag-enter", () => {
  $("drop-zone").classList.add("drag-over");
});
listen("tauri://drag-leave", () => {
  $("drop-zone").classList.remove("drag-over");
});
listen("tauri://drag-drop", (event) => {
  $("drop-zone").classList.remove("drag-over");
  const paths = event.payload && event.payload.paths;
  if (paths && paths.length) {
    if (!$("stage-pick").hidden) {
      pickFolder(paths[0]);
    }
  }
});

/* ---------------- stage 2: preview ---------------- */

let previewTimer = null;
let previewSeq = 0;

function enterPreviewStage() {
  const ins = state.inspection;
  $("proj-name").textContent = ins.name;
  $("proj-path").textContent = ins.path;
  $("proj-encoded").textContent = ins.encodedExisting || ins.encoded;
  const badge = $("encoded-badge");
  if (ins.encodedExisting) {
    badge.textContent = "history found";
    badge.className = "badge found";
    $("history-stats").hidden = false;
    $("stat-sessions").textContent = ins.history.sessionCount;
    $("stat-size").textContent = ins.history.totalSize;
    $("stat-modified").textContent = ins.history.lastModified || "—";
  } else {
    badge.textContent = "no history — never opened in Claude Code";
    badge.className = "badge missing";
    $("history-stats").hidden = true;
  }

  $("new-name").value = "";
  $("name-error").hidden = true;
  $("preview-body").hidden = true;
  $("rename-btn").disabled = true;
  state.preview = null;

  loadWarnings(ins.path);
  showStage("preview");
  $("new-name").focus();
}

async function loadWarnings(path) {
  $("warnings").innerHTML = "";
  let warnings = [];
  try {
    warnings = await invoke("get_warnings", { path });
  } catch { /* warnings are best-effort */ }
  $("warnings").innerHTML = warnings
    .map((w) => `<div class="warning-item">⚠ ${escapeHtml(w)}</div>`)
    .join("");
}

$("pick-other").addEventListener("click", () => {
  state.inspection = null;
  showStage("pick");
});

$("new-name").addEventListener("input", () => {
  $("rename-btn").disabled = true;
  clearTimeout(previewTimer);
  const value = $("new-name").value;
  if (value === "" || value === state.inspection.name) {
    $("name-error").hidden = true;
    $("preview-body").hidden = true;
    state.preview = null;
    return;
  }
  previewTimer = setTimeout(refreshPreview, 250);
});

$("deep-fix").addEventListener("change", () => {
  if (state.preview) renderPreview(state.preview);
});

async function refreshPreview() {
  const ins = state.inspection;
  const newName = $("new-name").value;
  const seq = ++previewSeq;
  let preview;
  try {
    preview = await invoke("preview_rename", {
      path: ins.path,
      newName,
      deepFix: $("deep-fix").checked,
    });
  } catch (e) {
    if (seq !== previewSeq) return;
    $("name-error").textContent = String(e);
    $("name-error").hidden = false;
    $("preview-body").hidden = true;
    return;
  }
  if (seq !== previewSeq) return; // a newer keystroke superseded this preview

  if (!preview.valid) {
    $("name-error").textContent = preview.error;
    $("name-error").hidden = false;
    $("preview-body").hidden = true;
    $("rename-btn").disabled = true;
    state.preview = null;
    return;
  }
  $("name-error").hidden = true;
  state.preview = preview;
  renderPreview(preview);
  $("preview-body").hidden = false;
  $("rename-btn").disabled = false;
}

function renderPreview(p) {
  const rows = [];
  const arrow = '<span class="diff-arrow">→</span>';

  rows.push(`<tr>
    <td class="diff-label">Disk folder</td>
    <td><span class="diff-old mono">${escapeHtml(p.oldPath)}</span>${arrow}<span class="diff-new mono">${escapeHtml(p.newPath)}</span>${p.caseOnly ? '<span class="diff-kind">case-only, two-step rename</span>' : ""}</td>
  </tr>`);

  if (p.encodedRenames.length === 0) {
    rows.push(`<tr>
      <td class="diff-label">History folders</td>
      <td class="muted">none found — this project was never opened in Claude Code, so the history steps will be skipped</td>
    </tr>`);
  } else {
    for (const r of p.encodedRenames) {
      const change =
        r.from === r.to
          ? `<span class="mono">${escapeHtml(r.from)}</span><span class="diff-kind">name unchanged — the new path encodes to the same folder</span>`
          : `<span class="diff-old mono">${escapeHtml(r.from)}</span>${arrow}<span class="diff-new mono">${escapeHtml(r.to)}</span><span class="diff-kind">${escapeHtml(r.kind)}</span>`;
      rows.push(`<tr>
        <td class="diff-label">History folder</td>
        <td>${change}</td>
      </tr>`);
    }
  }

  if (p.claudeJsonExists) {
    const parts = p.variantCounts
      .filter((v) => v.count > 0)
      .map((v) => `<div>${v.count} × <span class="mono">${escapeHtml(v.pattern)}</span></div>`)
      .join("");
    rows.push(`<tr>
      <td class="diff-label">.claude.json</td>
      <td>${p.totalMatches} replacement${p.totalMatches === 1 ? "" : "s"}${p.totalMatches ? ":" : " — nothing references this path"}${parts}</td>
    </tr>`);
  } else {
    rows.push(`<tr>
      <td class="diff-label">.claude.json</td>
      <td class="muted">not found — will be skipped</td>
    </tr>`);
  }

  const deepOn = $("deep-fix").checked;
  rows.push(`<tr>
    <td class="diff-label">Session files</td>
    <td>${deepOn
      ? `deep fix will scan ${p.deepFixFileCount} session file${p.deepFixFileCount === 1 ? "" : "s"} and update embedded paths`
      : '<span class="muted">deep fix is off — embedded paths will keep the old name</span>'}</td>
  </tr>`);

  $("diff-table").innerHTML = rows.join("");
  $("deep-count").textContent = p.deepFixFileCount;

  const uv = $("unverified-box");
  if (p.unverified.length) {
    uv.innerHTML =
      "<strong>Left alone (could not be verified as belonging to this project):</strong><br>" +
      p.unverified.map((u) => `<span class="mono">${escapeHtml(u)}</span>`).join("<br>");
    uv.hidden = false;
  } else {
    uv.hidden = true;
  }
}

/* ---------------- stage 3: execute + result ---------------- */

$("rename-btn").addEventListener("click", doRename);
$("retry-btn").addEventListener("click", doRename);

async function doRename() {
  const ins = state.inspection;
  const newName = $("new-name").value;
  $("rename-btn").disabled = true;
  $("retry-btn").disabled = true;
  let result;
  try {
    result = await invoke("execute_rename", {
      path: ins.path,
      newName,
      deepFix: $("deep-fix").checked,
    });
  } catch (e) {
    result = { ok: false, locked: false, rolledBack: false, error: String(e), summary: [], newPath: null };
  }
  $("rename-btn").disabled = false;
  $("retry-btn").disabled = false;
  state.result = result;
  renderResult(result);
  showStage("result");
  refreshUndoButton();
}

function renderResult(r) {
  $("mascot-success").hidden = !r.ok;
  $("mascot-error").hidden = r.ok;
  $("result-tip").hidden = !r.ok;
  $("open-folder-btn").hidden = !r.ok;
  $("copy-summary-btn").hidden = !r.ok;
  $("start-over-btn").hidden = !r.ok;
  $("retry-btn").hidden = !(!r.ok && r.locked);
  $("back-to-preview-btn").hidden = r.ok;

  if (r.ok) {
    $("result-title").textContent = "Renamed!";
    $("result-summary").innerHTML = r.summary
      .map((s) => `<li>${escapeHtml(s)}</li>`)
      .join("");
  } else {
    $("result-title").textContent = "That didn't work";
    $("result-summary").innerHTML = `<li>${escapeHtml(r.error || "Unknown error")}</li>`;
  }
}

$("open-folder-btn").addEventListener("click", () => {
  if (state.result && state.result.newPath) {
    invoke("open_in_explorer", { path: state.result.newPath }).catch(() => {});
  }
});

$("copy-summary-btn").addEventListener("click", async () => {
  const r = state.result;
  if (!r) return;
  const text = ["Reclaude rename summary", ...r.summary].join("\n");
  const ok = await copyText(text);
  flashButton($("copy-summary-btn"), ok ? "Copied!" : "Copy failed");
});

$("back-to-preview-btn").addEventListener("click", () => {
  loadWarnings(state.inspection.path);
  showStage("preview");
});

$("start-over-btn").addEventListener("click", () => {
  state.inspection = null;
  state.preview = null;
  state.result = null;
  showStage("pick");
});

/* ---------------- undo ---------------- */

async function refreshUndoButton() {
  let info = null;
  try {
    info = await invoke("last_manifest");
  } catch { /* no manifest */ }
  const btn = $("undo-open");
  if (info && !info.undone) {
    btn.hidden = false;
    btn.dataset.details = `Reverts the rename of "${info.oldPath}" → "${info.newPath}" made on ${info.timestamp}.`;
  } else {
    btn.hidden = true;
  }
}

$("undo-open").addEventListener("click", () => {
  $("undo-details").textContent = $("undo-open").dataset.details || "";
  $("undo-result").hidden = true;
  $("undo-result").textContent = "";
  $("undo-confirm").hidden = false;
  $("undo-cancel").textContent = "Cancel";
  $("modal-backdrop").hidden = false;
});

$("undo-cancel").addEventListener("click", () => {
  $("modal-backdrop").hidden = true;
});

$("undo-confirm").addEventListener("click", async () => {
  $("undo-confirm").disabled = true;
  const box = $("undo-result");
  try {
    const res = await invoke("undo_last");
    box.className = "ok";
    box.textContent = "Undone. " + res.summary.join(" · ");
    $("undo-confirm").hidden = true;
  } catch (e) {
    box.className = "err";
    box.textContent = String(e);
  }
  box.hidden = false;
  $("undo-confirm").disabled = false;
  $("undo-cancel").textContent = "Close";
  refreshUndoButton();
});

/* ---------------- init ---------------- */

refreshUndoButton();
showStage("pick");
