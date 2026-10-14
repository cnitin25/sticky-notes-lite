const { invoke } = window.__TAURI__.core;
const { getCurrentWindow } = window.__TAURI__.window;
const { listen } = window.__TAURI__.event;
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
  lockBtn: document.getElementById("lock-btn"),
  lockShackle: document.getElementById("lock-shackle"),
  confirmOverlay: document.getElementById("confirm-overlay"),
  confirmMessage: document.getElementById("confirm-message"),
  confirmOk: document.getElementById("confirm-ok"),
  confirmCancel: document.getElementById("confirm-cancel"),
};

let confirmOpen = false;

function showConfirm(message) {
  // Re-entry would stack a second pair of listeners on the one shared overlay,
  // and both copies would resolve on a single click.
  if (confirmOpen) return Promise.resolve(false);
  confirmOpen = true;
  return new Promise((resolve) => {
    el.confirmMessage.textContent = message;
    el.confirmOverlay.classList.add("open");
    // Focus the safe choice: a plain Enter then cancels rather than deletes.
    el.confirmCancel.focus();
    const cleanup = (result) => {
      confirmOpen = false;
      el.confirmOverlay.classList.remove("open");
      el.confirmOk.removeEventListener("click", onOk);
      el.confirmCancel.removeEventListener("click", onCancel);
      document.removeEventListener("keydown", onKey, true);
      resolve(result);
    };
    const onOk = () => cleanup(true);
    const onCancel = () => cleanup(false);
    const onKey = (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        cleanup(false);
      }
    };
    el.confirmOk.addEventListener("click", onOk);
    el.confirmCancel.addEventListener("click", onCancel);
    document.addEventListener("keydown", onKey, true);
  });
}

let saveTimer = null;
let pendingSave = false;
let loaded = false;
let currentColor = COLORS[0];
let wrapEnabled = false;
let pinned = false;
let locked = false;

// Shackle closed vs swung open, so the icon reads as a state, not just a button.
const SHACKLE_LOCKED = "M8 11V7a4 4 0 0 1 8 0v4";
const SHACKLE_UNLOCKED = "M8 11V7a4 4 0 0 1 7.5-2";

/// Width of the strip along the window edge left alone by drag-from-anywhere,
/// so the undecorated window's own edge resizing still gets the mousedown.
const RESIZE_EDGE_PX = 6;

/// Read-only covers everything that changes the note's text or look, plus
/// Delete. `readOnly` alone already blocks typing, Ctrl+V and drop-to-insert;
/// the buttons are disabled here and their handlers re-check `locked` too.
function applyLocked(enabled) {
  locked = enabled;
  el.title.readOnly = enabled;
  el.content.readOnly = enabled;
  el.colorBtn.disabled = enabled;
  el.pasteBtn.disabled = enabled;
  el.deleteBtn.disabled = enabled;
  if (enabled) el.palette.classList.remove("open");
  el.root.classList.toggle("locked", enabled);
  el.lockBtn.classList.toggle("active", enabled);
  el.lockShackle.setAttribute("d", enabled ? SHACKLE_LOCKED : SHACKLE_UNLOCKED);
  el.lockBtn.title = enabled ? "Unlock note" : "Lock note (read-only, drag from anywhere)";
}

/// A locked note has nothing to click into, so any plain left-press on it
/// starts a window drag -- except on controls, the textarea's scrollbars, and
/// the resize strip along the window edge.
function onLockedMouseDown(e) {
  if (!locked || e.button !== 0) return;
  if (e.target.closest("button, #palette, #confirm-overlay")) return;
  if (
    e.clientX < RESIZE_EDGE_PX ||
    e.clientY < RESIZE_EDGE_PX ||
    e.clientX >= window.innerWidth - RESIZE_EDGE_PX ||
    e.clientY >= window.innerHeight - RESIZE_EDGE_PX
  ) {
    return;
  }
  if (e.target === el.content && (e.offsetX >= el.content.clientWidth || e.offsetY >= el.content.clientHeight)) {
    return;
  }
  e.preventDefault();
  appWindow.startDragging().catch((err) => console.error(err));
}

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
  pendingSave = true;
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    flushSave();
  }, 500);
}

/// Write out immediately instead of waiting for the debounce. Called whenever
/// the note is about to stop being editable -- losing focus, being hidden, or
/// being closed -- since anything typed inside the debounce window would
/// otherwise go with the window.
async function flushSave() {
  clearTimeout(saveTimer);
  saveTimer = null;
  // Nothing typed since the last write, or the note hasn't loaded yet -- in
  // which case saving would push empty fields over the real note on disk.
  if (!pendingSave || !loaded) return;
  pendingSave = false;
  try {
    await save();
  } catch (err) {
    pendingSave = true;
    console.error(err);
  }
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

  // Registered before the note loads so a close arriving mid-load is still
  // answered -- flushSave() is a no-op until there is something to write.
  await listen("flush-save", () => {
    flushSave();
  });
  await listen("save-and-close", async () => {
    await flushSave();
    await invoke("close_note");
  });

  // Covers the paths the backend can't see: clicking away to another window,
  // and the webview being torn down.
  window.addEventListener("blur", () => {
    flushSave();
  });
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) flushSave();
  });
  window.addEventListener("pagehide", () => {
    flushSave();
  });

  const note = await invoke("load_note", { id: noteId });
  el.title.value = note.title;
  el.content.value = note.content;
  applyColor(note.color);
  applyWrap(note.wrap);
  applyPinned(note.pinned);
  applyLocked(note.locked);
  loaded = true;

  el.title.addEventListener("input", () => {
    appWindow.setTitle(el.title.value.trim() || "Note").catch(() => {});
    scheduleSave();
  });
  el.content.addEventListener("input", scheduleSave);

  el.colorBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    if (locked) return;
    el.palette.classList.toggle("open");
  });
  document.addEventListener("click", () => el.palette.classList.remove("open"));
  el.palette.addEventListener("click", (e) => e.stopPropagation());

  el.copyBtn.addEventListener("click", async () => {
    await writeText(el.content.value);
    flash(el.copyBtn);
  });

  el.pasteBtn.addEventListener("click", async () => {
    if (locked) return;
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
    // Only reflect the new state once the backend has actually applied it,
    // otherwise a failure leaves the button lit over an unpinned window.
    const next = !pinned;
    try {
      await invoke("set_pinned", { id: noteId, pinned: next });
      applyPinned(next);
    } catch (err) {
      console.error(err);
    }
  });

  el.lockBtn.addEventListener("click", async () => {
    // Persist first, as with pin, so a failure can't show a lock that isn't saved.
    const next = !locked;
    try {
      // Locking freezes the text, so write out anything still in the debounce.
      if (next) await flushSave();
      await invoke("set_locked", { id: noteId, locked: next });
      applyLocked(next);
    } catch (err) {
      console.error(err);
    }
  });

  el.root.addEventListener("mousedown", onLockedMouseDown);

  el.minimizeBtn.addEventListener("click", async () => {
    await flushSave();
    await invoke("minimize_note");
  });

  el.wrapBtn.addEventListener("click", () => {
    applyWrap(!wrapEnabled);
    scheduleSave();
  });

  el.deleteBtn.addEventListener("click", async () => {
    if (locked) return;
    if (await showConfirm("Delete this note permanently?")) {
      clearTimeout(saveTimer);
      pendingSave = false;
      await invoke("delete_note", { id: noteId });
    }
  });

  document.addEventListener("keydown", (e) => {
    if (confirmOpen) return;
    if (e.key === "Escape" && el.palette.classList.contains("open")) {
      el.palette.classList.remove("open");
      return;
    }
    if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === "n") {
      e.preventDefault();
      invoke("create_note").catch((err) => console.error(err));
    }
  });
}

init().catch(showFatalError);
