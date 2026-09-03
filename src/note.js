const { invoke } = window.__TAURI__.core;
const { getCurrentWindow } = window.__TAURI__.window;
const clipboard = window.__TAURI__.clipboardManager || {};
const writeText = clipboard.writeText || (() => Promise.reject(new Error("clipboard plugin unavailable")));
const readText = clipboard.readText || (() => Promise.reject(new Error("clipboard plugin unavailable")));

const appWindow = getCurrentWindow();
const noteId = new URLSearchParams(window.location.search).get("id");

const COLORS = [
  "#fff59d",
  "#f8bbd0",
  "#b3e5fc",
  "#c8e6c9",
  "#e1bee7",
  "#eeeeee",
  "#ffe0b2",
  "#ffffff",
];

const el = {
  root: document.getElementById("note"),
  title: document.getElementById("title"),
  content: document.getElementById("content"),
  colorBtn: document.getElementById("color-btn"),
  palette: document.getElementById("palette"),
  copyBtn: document.getElementById("copy-btn"),
  pasteBtn: document.getElementById("paste-btn"),
  newBtn: document.getElementById("new-btn"),
  pinBtn: document.getElementById("pin-btn"),
  minimizeBtn: document.getElementById("minimize-btn"),
  deleteBtn: document.getElementById("delete-btn"),
  wrapBtn: document.getElementById("wrap-btn"),
  confirmOverlay: document.getElementById("confirm-overlay"),
  confirmMessage: document.getElementById("confirm-message"),
  confirmOk: document.getElementById("confirm-ok"),
  confirmCancel: document.getElementById("confirm-cancel"),
};

function showConfirm(message) {
  return new Promise((resolve) => {
    el.confirmMessage.textContent = message;
    el.confirmOverlay.classList.add("open");
    const cleanup = (result) => {
      el.confirmOverlay.classList.remove("open");
      el.confirmOk.removeEventListener("click", onOk);
      el.confirmCancel.removeEventListener("click", onCancel);
      resolve(result);
    };
    const onOk = () => cleanup(true);
    const onCancel = () => cleanup(false);
    el.confirmOk.addEventListener("click", onOk);
    el.confirmCancel.addEventListener("click", onCancel);
  });
}

let saveTimer = null;
let currentColor = COLORS[0];
let wrapEnabled = false;
let pinned = false;

function applyPinned(enabled) {
  pinned = enabled;
  el.pinBtn.classList.toggle("active", enabled);
}

function applyWrap(enabled) {
  wrapEnabled = enabled;
  el.content.classList.toggle("no-wrap", !enabled);
  el.wrapBtn.classList.toggle("active", enabled);
}

function applyColor(color) {
  currentColor = color;
  el.root.style.setProperty("--note-color", color);
}

function scheduleSave() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(save, 500);
}

async function save() {
  await invoke("save_note", {
    fields: {
      id: noteId,
      title: el.title.value,
      content: el.content.value,
      color: currentColor,
      wrap: wrapEnabled,
    },
  });
}

function flash(button) {
  button.classList.add("flash");
  setTimeout(() => button.classList.remove("flash"), 300);
}

function buildPalette() {
  el.palette.innerHTML = "";
  for (const color of COLORS) {
    const swatch = document.createElement("button");
    swatch.type = "button";
    swatch.className = "swatch";
    swatch.style.background = color;
    swatch.addEventListener("click", () => {
      applyColor(color);
      el.palette.classList.remove("open");
      scheduleSave();
    });
    el.palette.appendChild(swatch);
  }
}

function showFatalError(err) {
  console.error(err);
  el.root.innerHTML = `<div style="padding:12px;font:12px monospace;white-space:pre-wrap;color:#900;background:#fff;">Note failed to load:\n${String(
    err && err.message ? err.message : err
  )}</div>`;
}

async function init() {
  buildPalette();

  const note = await invoke("load_note", { id: noteId });
  el.title.value = note.title;
  el.content.value = note.content;
  applyColor(note.color);
  applyWrap(note.wrap);
  applyPinned(note.pinned);

  el.title.addEventListener("input", () => {
    appWindow.setTitle(el.title.value.trim() || "Note").catch(() => {});
    scheduleSave();
  });
  el.content.addEventListener("input", scheduleSave);

  el.colorBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    el.palette.classList.toggle("open");
  });
  document.addEventListener("click", () => el.palette.classList.remove("open"));
  el.palette.addEventListener("click", (e) => e.stopPropagation());

  el.copyBtn.addEventListener("click", async () => {
    await writeText(el.content.value);
    flash(el.copyBtn);
  });

  el.pasteBtn.addEventListener("click", async () => {
    const text = await readText();
    if (typeof text === "string" && text.length > 0) {
      el.content.value = text;
      scheduleSave();
      flash(el.pasteBtn);
    }
  });

  el.newBtn.addEventListener("click", async () => {
    await invoke("create_note");
  });

  el.pinBtn.addEventListener("click", async () => {
    applyPinned(!pinned);
    await invoke("set_pinned", { id: noteId, pinned });
  });

  el.minimizeBtn.addEventListener("click", async () => {
    await invoke("minimize_note");
  });

  el.wrapBtn.addEventListener("click", () => {
    applyWrap(!wrapEnabled);
    scheduleSave();
  });

  el.deleteBtn.addEventListener("click", async () => {
    if (await showConfirm("Delete this note permanently?")) {
      clearTimeout(saveTimer);
      await invoke("delete_note", { id: noteId });
    }
  });
}

init().catch(showFatalError);
