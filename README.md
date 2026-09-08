# Sticky Notes Lite

A lightweight, fully offline sticky-notes app for Windows, built with Tauri v2 (Rust backend + vanilla HTML/CSS/JS frontend, no bundler). Each note is its own draggable, resizable OS window; there's no visible main window — the app lives entirely in the system tray. Notes persist as individual JSON files, auto-saved as you type.

## Features

- Separate OS windows per note — drag/resize independently, anywhere on screen (including across monitors)
- Per-note editable title, background color (fixed 8-color palette), and text-wrap toggle
- **Text wrap is off by default, on purpose** — a new note is created with `wrap: false`, and a note file predating the field is now read the same way (it used to default to `true`, disagreeing with both the intended behaviour and this list). Notes here are usually pasted SQL/logs/paths where wrapping hurts more than it helps; the footer's **Wrap** button turns it on per note and is persisted
- **Spell-check is disabled** on both the title and the body (`spellcheck="false"` in `note.html`). WebView2 ships Chromium's spellchecker and enables it on every editable field by default, which red-underlines most of what actually gets pasted into these notes. There is no per-note toggle — if one is ever wanted it is `el.content.spellcheck = ...`, persisted alongside `wrap`
- One-click **copy** (copies note content only, not the title) and **paste**
- **Paste replaces the entire note, deliberately** — this is the intended behaviour, not an oversight. The button exists to make a note *be* whatever is on the clipboard; ordinary Ctrl+V is still available for inserting at the caret. Two consequences, both accepted: it does not confirm the way Delete does, and because it assigns `el.content.value` directly it also clears the textarea's native undo stack, so Ctrl+Z will not bring the previous content back. Don't "fix" this into an insert-at-caret — that is what Ctrl+V already does
- **Minimize**: hides the note window, and the state is **persisted** — a note minimized when the app closes stays minimized on next launch instead of reappearing on screen. Bring one back from the tray's **Restore** submenu
- **Pin**: keeps a note always on top of every other window (not just other notes) — persisted, so a pinned note reopens pinned
- **New note** button on every note's toolbar (in addition to the tray's "New Note") — a fresh note isn't focused automatically (click into it before typing); this is deliberate, see "Notable bugs fixed" below
- Delete with an in-page confirmation dialog (not the browser's native `confirm()`, which clips on small windows)
- Auto-save (debounced ~500ms) to `%APPDATA%\com.stickynoteslite.desktop\notes\<uuid>.json`, plus an immediate flush whenever the note stops being editable — losing focus, being hidden/minimized, being closed, or the tray's Quit (see "Unsaved text could be lost on close or Quit" below)
- Position/size auto-saved on drag/resize — coalesced in memory and written at most every ~400ms from a background thread, not once per frame — restored on next launch, with a safety clamp so a note can never reopen fully off-screen
- Keyboard: **Ctrl+N** new note, **Esc** closes the color palette or cancels the delete dialog (which opens with *Cancel* focused, so a stray Enter can never delete)
- **Single instance**: launching the exe while it is already running does not start a second copy -- the new process exits and the running one re-shows any note that isn't minimized (deliberately-minimized notes stay hidden)
- No taskbar icon (tray-only); system tray menu: **New Note**, **Restore ▸**, **Quit**. The Restore submenu lists **All Notes** plus one entry per currently-minimized note, labelled by its title (or first non-blank line, or `(empty note)`), so a single note can be brought back without unhiding the rest
- If *every* note is minimized the app legitimately starts with no windows at all — it is tray-resident, and forcing one open would defeat the point of having minimized them
- Quitting via the tray actually exits (as opposed to the Tauri default of quitting whenever the last window closes, which would fight against the tray-resident design)

## Project layout

```
src-tauri/           Rust backend
  src/lib.rs           window management, tray, Tauri commands
  src/notes.rs         Note struct + JSON read/write (the data layer)
  tauri.conf.json      no default window (app.windows: []) — windows are all spawned dynamically
  capabilities/        default.json grants note-*  windows the permissions they need
src/                 Frontend (vanilla JS, one template reused per note)
  note.html / note.css / note.js
dist/                The actual exe to run/autostart from (see "Building" below)
```

## Development

```powershell
npm install
npm run tauri dev
```

Every note window loads `note.html?id=<uuid>`, reading the id from the URL query string.

## Building a release exe

```powershell
.\build-release.ps1            # build + sign
.\build-release.ps1 -Deploy    # build + sign + swap into dist\ and relaunch
.\build-release.ps1 -SkipSign  # build only
```

**Use the script, not a bare `npm run tauri build`, for anything you actually ship.** A plain build bakes build-machine paths into the binary; the script passes the `--remap-path-prefix` flags that keep them out (see "What the release build strips out" below). It also clears the build-script cache, which is required whenever the icon changes.

A bare `npm run tauri build` still works for a throwaway local build, and adding `-- --no-bundle` skips the MSI/NSIS installers (nothing here uses them -- the app runs from `dist\`). It produces `src-tauri\target\release\sticky-notes-lite.exe` plus, without `--no-bundle`, installers under `src-tauri\target\release\bundle\`. `target\` is disposable build cache (can grow to several GB) — safe to delete, `npm run tauri build` regenerates it from scratch (a few minutes) whenever needed.

**Frontend changes need a full rebuild too** — in a release build, `src/*.html|css|js` get embedded into the compiled binary; editing them on disk has no effect until you rebuild (unlike `tauri dev`, which serves them live).

## What the release build strips out

A default release build leaks the build machine into the shipped binary. Measured on a real build, before this was addressed:

- a **CodeView debug directory entry** naming `sticky_notes_lite.pdb`, and
- **536 absolute local paths** -- 370 `C:\Users\<you>\.cargo\registry\src\...` and 165 `C:\Users\<you>\.rustup\toolchains\...`.

The important detail is *where* those paths live: **`.rdata`, not debug info.** They are `panic!` location strings -- rustc bakes the absolute source path of every crate into the `Location` records behind `unwrap()`, bounds checks and friends. So `strip` does not remove them and neither does anything else that only touches debug data. Only `--remap-path-prefix` does.

Two independent fixes, therefore:

| Leak | Fix | Where |
|---|---|---|
| CodeView entry / `.pdb` reference | `strip = true` plus `-Clink-arg=/DEBUG:NONE` | `[profile.release]` in `src-tauri/Cargo.toml`, and `build-release.ps1` |
| 536 `.cargo` / `.rustup` paths in `.rdata` | `--remap-path-prefix` | `build-release.ps1` |

`build-release.ps1` derives the prefixes from `CARGO_HOME` / `RUSTUP_HOME` (falling back to `%USERPROFILE%`) rather than hardcoding them, so no username is committed to the repo. Result, verified by scanning the binary: **0** occurrences of the username in ASCII or UTF-16, and 536 `[cargo]` / `[rustup]` / `[src]` markers in their place.

`trim-paths` in `[profile.release]` would be the tidier fix, but it is **not stabilized in Cargo 1.98** -- it fails the manifest parse with "feature `trim-paths` is required". Revisit when it stabilizes and the script's remap flags can go away.

What deliberately remains:

- **67 `/rustc/<hash>/library/...` paths.** These come from the precompiled standard library, which the Rust project already remaps; they describe nothing about this machine.
- **A 1028-byte `POGO` debug directory entry.** Linker optimization metadata containing no paths.

**Trade-off**: no `.pdb` is produced at all now, so a future crash dump resolves only to module+offset. That is exactly what the deadlock investigation described below worked from, so this costs nothing that was being relied on -- but if you ever need full symbols, build once with `strip = false` and without `/DEBUG:NONE`, and keep that `.pdb` locally rather than shipping it.

## Code signing (this machine has Smart App Control enabled)

Windows 11 Smart App Control (SAC) blocks unsigned executables outright — no "Run anyway" override, unlike classic SmartScreen. Every new build needs re-signing, since each build is a new file hash.

### Current certificate (already created and trusted on this machine)

- Subject: `CN=Sticky Notes Lite (Local)`
- Thumbprint: `2FD74642CE56E472D2CB807D396B77DA651FEF41`
- Store: `Cert:\CurrentUser\My` (private key), trusted via `LocalMachine\Root` + `LocalMachine\TrustedPublisher`
- Valid until 2036

`build-release.ps1` does this for you. To sign by hand:

```powershell
$signtool = "C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\signtool.exe"
& $signtool sign /fd SHA256 /sha1 "2FD74642CE56E472D2CB807D396B77DA651FEF41" /tr http://timestamp.digicert.com /td SHA256 "src-tauri\target\release\sticky-notes-lite.exe"
```

Then copy the signed exe to `dist\` — this is the stable location to actually run/autostart from; don't run directly out of `target\release\`, since that gets wiped whenever `target\` is cleaned up:

```powershell
Copy-Item "src-tauri\target\release\sticky-notes-lite.exe" "dist\sticky-notes-lite.exe" -Force
```

### If the certificate is ever lost and needs recreating

Two ways to generate a fresh signing certificate — either works; the PowerShell method below is what's currently in use.

**Option A — `New-SelfSignedCertificate` (PowerShell, no extra tools needed):**

```powershell
$cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject "CN=Sticky Notes Lite (Local)" `
    -CertStoreLocation "Cert:\CurrentUser\My" -NotAfter (Get-Date).AddYears(10) `
    -KeyUsage DigitalSignature -KeyAlgorithm RSA -KeyLength 2048
$cert.Thumbprint   # note this down — you'll need it for every future signtool call

Export-Certificate -Cert $cert -FilePath "sticky-notes-lite-signing.cer"
```

Then, in an **elevated** PowerShell (adding to the Root store from a script requires elevation and, even then, `Import-Certificate` throws "UI is not allowed" — use `certutil` instead, which doesn't hit that restriction):

```powershell
certutil -f -addstore "Root" "sticky-notes-lite-signing.cer"
certutil -f -addstore "TrustedPublisher" "sticky-notes-lite-signing.cer"
```

**Option B — `mkcert` + OpenSSL (if you'd rather use an existing local dev CA):**

[mkcert](https://github.com/FiloSottile/mkcert) is a tool for generating locally-trusted certificates — normally for HTTPS/TLS on localhost, not code signing, but its local CA can be reused to issue a code-signing certificate too, via OpenSSL. This machine already has both installed (`mkcert` via Chocolatey, OpenSSL via Git for Windows), and a mkcert CA already exists and is trusted. If starting from scratch elsewhere:

```powershell
# Install mkcert (pick one)
winget install FiloSottile.mkcert
# or: choco install mkcert
# or download the binary from https://github.com/FiloSottile/mkcert/releases and put it on PATH

# Create + trust a local CA (installs into Windows' Root store, and Firefox's if present)
mkcert -install

# Find where the CA files live
mkcert -CAROOT
# -> e.g. C:\Users\<you>\AppData\Local\mkcert\rootCA.pem / rootCA-key.pem
```

mkcert itself only issues TLS server/client certs, not code-signing ones — to get a codeSigning-EKU leaf certificate chained to that already-trusted CA, use OpenSSL directly against the CA files mkcert created:

```powershell
cd (mkcert -CAROOT)   # or wherever rootCA.pem / rootCA-key.pem live

# 1. Generate a private key for the new signing cert
openssl genrsa -out codesign-key.pem 2048

# 2. Write a minimal OpenSSL config requesting the codeSigning EKU
@'
[req]
distinguished_name = dn
req_extensions = ext
prompt = no

[dn]
CN = Sticky Notes Lite (mkcert)

[ext]
keyUsage = digitalSignature
extendedKeyUsage = codeSigning
'@ | Set-Content codesign.cnf

# 3. Create the CSR
openssl req -new -key codesign-key.pem -out codesign.csr -config codesign.cnf

# 4. Sign it with the mkcert CA (produces a leaf cert chained to the already-trusted CA)
openssl x509 -req -in codesign.csr -CA rootCA.pem -CAkey rootCA-key.pem -CAcreateserial `
    -out codesign-cert.pem -days 3650 -extfile codesign.cnf -extensions ext

# 5. Bundle into a .pfx (PowerShell/signtool need the private key alongside the cert)
openssl pkcs12 -export -out codesign.pfx -inkey codesign-key.pem -in codesign-cert.pem -passout pass:changeit

# 6. Import into your personal certificate store
Import-PfxCertificate -FilePath codesign.pfx -CertStoreLocation Cert:\CurrentUser\My `
    -Password (ConvertTo-SecureString -String "changeit" -AsPlainText -Force)
```

Since the mkcert CA is already trusted machine-wide, this leaf certificate is trusted too without any further `certutil` step — just grab its thumbprint (`Get-PfxCertificate codesign.pfx`) and use it with `signtool sign /sha1 <thumbprint> ...` the same way as Option A.

**Caveat that applies to *either* option**: see the next section — Smart App Control's acceptance of *any* self-signed/locally-trusted certificate (mkcert-based or not) has proven unpredictable in practice. Neither method produces a certificate that reaches the "Enterprise" Code Integrity signing level Windows actually wants; both just happen to work often enough to be worth using over nothing.

### Smart App Control is unpredictable even when correctly signed

The exact same signing recipe (same certificate, same elevated PowerShell) has both succeeded and failed across different builds of this app. Checking the reason via:

```powershell
Get-WinEvent -LogName "Microsoft-Windows-CodeIntegrity/Operational" -MaxEvents 10 |
    Where-Object { $_.Message -match "sticky-notes-lite" } | Select-Object TimeCreated, Id, Message
```

...consistently shows:

```
Code Integrity determined that a process ... attempted to load sticky-notes-lite.exe
that did not meet the Enterprise signing level requirements or violated code
integrity policy (Policy ID: {0283ac0f-fff1-49ae-ada1-8a933130cad6}).
```

"Enterprise signing level" is a specific Windows Code Integrity classification that a self-signed certificate — from `New-SelfSignedCertificate`, mkcert, or otherwise — cannot structurally satisfy on its own. It appears SAC sometimes grants temporary leniency to a brand-new file hash from an already-trusted signer, and sometimes doesn't; this is not about shell choice or elevation (both have been tried in both winning and losing combinations).

**If a freshly signed build gets blocked, the fix that has worked**: turn Smart App Control off, then back on (Settings → Privacy & security → Windows Security → App & browser control → Smart App Control). This clears the stuck per-file block. Contrary to older guidance, **this does not require a full Windows reinstall/reset** — toggling it off and back on in place has worked fine on this machine (confirmed 2026-09-03).

**Reliable fallback while troubleshooting**: `npm run tauri dev` has never once hit this issue across the entire development of this app, since it's launched via `cargo run` rather than as a standalone signed binary.

**On another machine**: whether this friction shows up at all depends on whether *that* machine has Smart App Control enabled (it's opt-in even on clean Windows 11 installs). Confirmed on a second laptop without SAC: the exe just shows a normal SmartScreen "unrecognized publisher" prompt with a working "Run anyway" — no certificate setup needed there at all. If SAC-blocked friction on other machines becomes a recurring problem, look into **Azure Trusted Signing** (~$10/month, a real broadly-recognized certificate without the overhead of Microsoft Store submission) rather than continuing to fight self-signing per machine.

## Changing the app icon

Everything under `src-tauri/icons/` is **generated** -- don't hand-edit it. Supply a square source PNG (1024x1024 or larger, with transparency; the current icon came from a 1254x1254 one) and regenerate with Tauri's own generator:

```powershell
npm run tauri icon path\to\your-icon.png
Remove-Item -Recurse -Force src-tauri\icons\android, src-tauri\icons\ios   # Windows-only app
```

The source PNG is deliberately **not** kept in the repo -- it is only needed when the icon changes, and `icon.png` (512x512) plus `icon.ico` (256x256 entry) are already committed if a rough regeneration is ever needed. Keep the full-resolution original somewhere outside the project if you may want to re-derive at full quality.

**The build will silently keep the old icon unless you also delete the build-script cache.** This cost a full debug cycle once, so it is worth stating plainly:

```powershell
Remove-Item -Recurse -Force src-tauri\target\release\build\sticky-notes-lite-*
npm run tauri build -- --no-bundle
```

Why: `tauri-build` writes an `out/resource.rc` that references `icons\icon.ico` **by path**, and `embed-resource` compiles it to `out/resource.lib`. Neither cargo nor `embed-resource` tracks the *contents* of the referenced `.ico`, so once `resource.lib` exists it is reused even though the icon changed underneath it. `cargo clean -p sticky-notes-lite` does **not** reliably remove that directory either -- it was observed leaving a `resource.lib` two builds stale. Deleting the crate's build directory is what actually forces the resource to recompile.

**Explorer will keep showing the old icon after a correct rebuild.** This is Windows' per-path icon cache, not a build problem -- the exe is fine, the shell is lying. `ie4uinit.exe -show` refreshes it without restarting Explorer; the heavier fix (kill Explorer, delete `%LocalAppData%\Microsoft\Windows\Explorer\iconcache_*.db`, restart) is rarely needed and would also drop the app's tray icon until it is relaunched. The tray icon updates immediately on relaunch either way, so "tray icon changed but the exe icon didn't" is the signature of this, not of a failed build.

To verify an icon actually shipped rather than trusting the build (Explorer and `ExtractAssociatedIcon` will both happily show you a cached icon), check that the `.ico`'s own bytes are inside the exe. Take the signature from a **high-entropy** slice -- the 256x256 entry is PNG-compressed and works well. A slice from a mostly-transparent region is nearly all zero bytes and will match any binary, producing a false positive:

```python
# 256x256 entry of icon.ico, middle 128 bytes, must appear in the exe
sig in open("dist/sticky-notes-lite.exe", "rb").read()
```

**On icon size and RAM**: Windows embeds `icon.ico`, and Tauri decodes it once into a raw RGBA buffer for `default_window_icon` -- which this app reuses for the tray icon (`tray.icon(icon.clone())`), so it is not paid twice. The cost is the largest entry: 256x256 = 256 KB resident, one allocation at startup. Shipping the 1254x1254 source directly as the window icon would instead cost ~6 MB, which is the real reason to generate proper sizes. Measured private working set was 5.56 MB before the icon change and 4.58 MB after -- the icon is far below run-to-run variance.

Note `icon.icns` is macOS-only and never embedded in a Windows build; regenerating grew it from 98 KB to 1.52 MB of repo size for a file this app never reads. Drop it from `bundle.icon` if the repo size matters more than keeping the cross-platform option open.

## Data location

Notes live in `%APPDATA%\com.stickynoteslite.desktop\notes\<uuid>.json` — independent of where the `.exe` runs from. Moving/renaming/copying the exe never affects existing notes; only changing `identifier` in `src-tauri\tauri.conf.json` would (don't).

Note JSON schema: `{ id, title, content, color, position: {x,y}, size: {width,height}, wrap, pinned, minimized, created_at, updated_at }` — all in logical (DPI-independent) pixels for position/size. `wrap`, `pinned` and `minimized` all default to `false` when absent from an older file.

`id` is a UUID and doubles as the filename, so `notes.rs` validates it with `Uuid::parse_str` before building any path — a malformed or crafted id can never address a file outside the notes directory.

Writes land in `<uuid>.json.tmp` and are then renamed over the real file, so an interrupted write leaves the previous version intact instead of a truncated one. A note that still fails to parse is skipped by `list_notes()` **and logged to stderr** — it used to disappear from the UI with no trace at all.

Every read-modify-write of a note file goes through one process-wide lock (`NOTE_IO` in `notes.rs`). Content saves arrive on the main thread as IPC commands while geometry is flushed from a background thread, so without it a note dragged during a content save could have the typed text written back from a stale copy. The lock is per-process, and that is sufficient *because* of the single-instance guard below -- there is only ever one process writing. Don't remove the guard on the assumption `NOTE_IO` covers this; it does not.

## Autostart on login

Drop a shortcut to `dist\sticky-notes-lite.exe` into the Startup folder (`Win+R` → `shell:startup`).

## Notable bugs fixed during development (for future reference)

- **Quit didn't actually quit**: Tauri's default is to exit the whole app when the last window closes, which fights the tray-resident design — fixed by intercepting `RunEvent::ExitRequested` and only allowing it through when the tray's "Quit" set an explicit flag first (`lib.rs`, `AppState.quitting`). Without this distinction, the veto needed to keep the app alive after closing the last note window also silently swallowed genuine Quit requests, leaving the app half-torn-down (visible, unresponsive).
- **Notes could reopen fully off-screen**: `WindowEvent::Moved`/`Resized` deliver *physical* pixels, but `WebviewWindowBuilder::position()`/`inner_size()` expect *logical* ones — on a DPI-scaled display (this machine: 125%), saved positions drifted by the scale factor, eventually pushing a note's saved position past the screen edge with no way to get it back (no taskbar entry to help). Fixed two ways: convert physical→logical (divide by `window.scale_factor()`) before saving geometry, and clamp any restored position to keep at least the title bar on some connected monitor (`clamp_to_visible_area()`), so this class of bug can't recur even from a future monitor/resolution change.
- **Delete confirmation dialog clipped on small notes**: the browser's native `confirm()` renders as a dialog sized to/anchored within the webview bounds in WebView2, clipping on a small note. Replaced with a custom in-page overlay (`#confirm-overlay`) sized relative to the note itself.
- **Creating/showing a note window could freeze the whole app** — this was the hardest bug in the project and worth documenting in full since the root cause was easy to misdiagnose. Symptom: clicking "New Note" (from the "+" button or the tray), "Show All Notes", or pinning-then-creating-a-note would sometimes hang the entire app (all windows unresponsive, tray icon dead).
  - **Root cause, found via an actual memory dump** (captured with Sysinternals `procdump -ma`, then walked in Python using the `minidump` package since no native debugger was installed — see git history/scratch scripts if this recurs and needs re-diagnosing): a genuine cross-thread deadlock between our main thread and a WebView2-internal compositor thread, both blocked in their own `win32u.dll` syscall waiting on each other. Two distinct triggers fed into this one deadlock class:
    1. `open_note_window`'s "already open, bring it back" path called both `.show()` **and** `.set_focus()` on every note — "Show All Notes" did this back-to-back for multiple windows with no yielding, and forcing focus on several WebView2-hosted windows in a tight loop is enough to trigger it. **Fix**: dropped `.set_focus()`; `.show()` alone is sufficient to un-hide a note.
    2. A newly built window auto-focuses itself by default as part of `WebviewWindowBuilder::build()`, which is itself another focus-activation event that can collide with a concurrent one (another window's own creation, or the tray icon's own activation requirements). **Fix**: `.focused(false)` on every new note window. Trade-off: a brand-new note doesn't have keyboard focus immediately — one extra click is needed before typing. Worth it for eliminating this deadlock class entirely.
  - **A second, independent contributing cause**: `AppHandle::run_on_main_thread`'s name is misleading. Checked directly against the vendored source in the Cargo registry cache (`~/.cargo/registry/src/.../tauri-runtime-wry-2.11.4/src/lib.rs`, `send_user_message`): if the calling thread *is already* the main thread, it runs the closure **immediately, inline, synchronously** — it does not defer to the next event-loop tick. Since this backend's IPC command handlers already execute directly on the main thread, wrapping `create_note`'s window-building in `run_on_main_thread` (an earlier, incomplete fix attempt) was a no-op: it still built the new window reentrantly, in the middle of dispatching the very IPC message that invoked the command, still colliding with WebView2's own message pumping. **Fix**: force genuine deferral by calling `run_on_main_thread` from a throwaway `std::thread::spawn` instead of directly — from a non-main thread, `send_user_message` takes its other branch and posts a real event to tao's event loop queue, processed on the next iteration rather than reentrantly. Applied to `create_note` and both tray menu handlers ("New Note", "Show All Notes").
  - **Lesson for future debugging of hangs in this app**: don't guess from symptoms alone — a real memory dump (`procdump -ma sticky-notes-lite.exe dump.dmp`, no admin/install needed, downloadable standalone from `https://live.sysinternals.com/procdump64.exe`) analyzed with the `minidump` Python package (`pip install minidump`) gives thread-by-thread instruction pointers resolved to module+offset, which is usually enough to identify a deadlock's shape even without full symbols/PDBs.

- **Dragging a note rewrote its JSON on every frame, on the main thread**: `WindowEvent::Moved`/`Resized` fire roughly once per frame for the whole duration of a drag or resize, and each one called `update_note_geometry()`, which is a complete read → `serde_json` parse → `to_string_pretty` → write cycle. A two-second drag was therefore hundreds of full file rewrites, all of them blocking the same main thread that pumps WebView2 — the identical thread that the deadlock above was traced to. The content path had been debounced ~500ms in JS since the start; the geometry path had no debounce at all. **Fix**: `Moved`/`Resized` now only accumulate into `AppState.pending_geometry` (an in-memory `HashMap<note id, PendingGeometry>`, so repeated events for one note collapse into one pending entry), and a background thread drains it to disk every `GEOMETRY_FLUSH_MS` (400ms). `flush_geometry()` is also called explicitly at the points where waiting would lose data: on minimize, on close, on tray Quit, and on `ExitRequested`. Worst case on a hard kill is now ~400ms of position drift instead of a stalled UI on every drag.
  - **This introduced a race that had not existed before**, and the fix is the reason `NOTE_IO` exists. Geometry writes now happen on a background thread while content saves still arrive on the main thread as IPC commands, so two full read-modify-write cycles against the same file can genuinely interleave — dragging a note while its content save is in flight could write the typed text back from a stale in-memory copy. Every note-file mutation in `notes.rs` now takes one process-wide `NOTE_IO` mutex. Before this change everything ran on the main thread and was serialized by accident, not by design; don't remove the lock on the assumption that's still true.

- **Unsaved text could be lost on close or Quit**: `scheduleSave()` was the only path from the textarea to disk, on a 500ms trailing debounce, and nothing flushed it when a window went away. Type a line, hit Alt+F4 or tray → Quit, and it was gone — `app.exit(0)` terminated the process outright and window destruction took the webview (and the only copy of the text) with it. **Fix**, in three parts:
  1. Frontend `flushSave()` writes immediately instead of waiting out the debounce, and runs on `blur`, `visibilitychange` (hidden), `pagehide`, and before minimize. It is a no-op unless something is actually dirty *and* the note has finished loading — the `loaded` guard matters, since flushing before `load_note` resolves would push empty fields over the real note on disk.
  2. Closing is now a handshake. `CloseRequested` calls `api.prevent_close()`, emits `save-and-close` to that one window (`emit_to`, not `emit` — a broadcast would close every note), and the frontend saves and then calls the new `close_note` command, which marks the label in `AppState.close_allowed` so the second `CloseRequested` passes straight through. A watchdog thread force-closes after `CLOSE_WATCHDOG_MS` (800ms) regardless: **a note that refuses to close because its webview is wedged would be a far worse bug than losing the last few keystrokes.** `delete_note` marks `close_allowed` itself and skips the handshake — there is nothing left on disk to save into.
  3. Tray Quit broadcasts `flush-save` to every note, waits `QUIT_FLUSH_GRACE_MS` (250ms) for the IPC round trips to land, flushes geometry, and only then sets `quitting` and exits. The wait runs on a throwaway thread, since sleeping on the main thread would block the very event loop those saves travel over.

- **`AppState.open_ids` was dead state**: inserted on window creation and removed in two places, but never read anywhere — `open_note_window()` has always used `app.get_webview_window(&label)` as its actual source of truth. Removed along with its three lock sites, to stop it reading like a mechanism that something depends on.

- **`wrap` defaulted two different ways**: `Note.wrap` deserialized a missing value as `true` (`#[serde(default = "default_true")]`) while `create_note_default()` sets `false` and the feature list documents "default off". Only reachable for a note file predating the field, but it made the intended default ambiguous to anyone reading the struct. Now `#[serde(default)]` — false, matching a freshly created note.

- **Smaller hardening done in the same pass** (none of these were live bugs; all were one-line-ish reachable failure modes):
  - `note_path()` validates the id with `Uuid::parse_str` before building a path. The id arrives from the window's own URL so nothing untrusted reached it, but `delete_note` on a `../`-bearing id would have removed an arbitrary file — better structurally impossible than incidentally safe.
  - Note writes are now temp-file-plus-rename, and `list_notes()` logs what it skips instead of silently dropping unparseable files.
  - `notes_dir()` returns `Result` instead of `expect()`-ing — it is called from every command, so it was an abort path rather than an error path.
  - A CSP is set in `tauri.conf.json` (it was `null`, i.e. disabled). `script-src 'self'` is the part that matters; `style-src` keeps `'unsafe-inline'` because `showFatalError()` injects a `style=` attribute and styling is not the attack surface here.
  - Pin no longer updates the button optimistically — it awaits `set_pinned` and only then reflects the new state, so a failure can't leave the button lit over an unpinned window.
  - The delete dialog handles Esc, opens with **Cancel** focused (so Enter cancels rather than deletes), and guards against being opened twice — a second open used to stack another pair of listeners on the one shared overlay, and both copies resolved on a single click.

- **Minimized notes reopened on every launch**: minimize only called `window.hide()`, which is process-local state, so a restart had no idea a note had been hidden and rebuilt a window for all of them. `Note.minimized` now persists it. The startup pass skips minimized notes entirely rather than building a hidden window for each — cheaper, and it keeps `open_note_window()` as the single place that un-minimizes (it clears the flag, then either un-hides an existing window or builds one on demand). `update_note_minimized()` is a no-op when the flag already matches, so the startup pass and ordinary restores don't rewrite files that have nothing to change.

- **The tray's "Show All Notes" became a "Restore" submenu**: **All Notes** (the old behaviour), a separator, then one entry per currently-minimized note. Two things worth knowing if you touch this:
  - **The OS builds a tray menu once**, so the Restore list has to be rebuilt whenever the set of minimized notes changes. `refresh_tray_menu()` does that and is called from minimize, from each restore, and from delete (a deleted note may have been in the list). It must only be called on the main thread — from a command handler, or inside a `run_on_main_thread` closure — for the same reasons documented in the deadlock notes above. `TrayIconBuilder::with_id(TRAY_ID)` exists purely so `app.tray_by_id()` can find the icon again to call `set_menu()`.
  - **Menu ids are matched by prefix**, so `RESTORE_PREFIX` is `restore-note-` while "All Notes" is `restore-all` — deliberately *not* sharing the prefix, so the catch-all arm's `strip_prefix()` can't swallow it. Note labels get `&` doubled, since Windows menus read a single `&` as a mnemonic marker and would eat it.

- **Every launch opened a duplicate set of note windows**: there was no single-instance guard, so each process independently ran `list_notes()` and opened a window per non-minimized note. `app.get_webview_window(label)` only sees the calling process's own windows, so a second instance had no idea the first already had note X open. The duplicates landed at **identical coordinates** -- both processes read position from the same JSON -- so three notes launched twice looked like three windows, not six, and the twins were only discoverable by dragging one aside.
  - **What was and wasn't at risk**, since this is easy to over- or under-state. Not at risk: file corruption (writes are temp-file-then-rename), deleted-note resurrection (`update_note_fields` reads first and errors out if the file is gone), or clobbering from an untouched duplicate (a stale window only writes when `pendingSave` is set, so it stays silent even through the Quit flush). Genuinely at risk: **lost updates** -- `NOTE_IO` is a process-local `static Mutex` and does nothing across processes, and `update_note_fields` replaces title/content wholesale, so typing into note X in one instance and then into its pixel-aligned twin in the other silently discarded the first one's text. Made more likely by the app being tray-only with no taskbar entry to remind you it is already running.
  - **Fix**: `tauri-plugin-single-instance`, registered **before every other plugin** as its docs require. Its callback runs in the already-running instance; it re-shows notes where `minimized` is false and leaves minimized ones alone, so a relaunch never undoes a deliberate minimize. The plugin keys on `app.config().identifier`, not the exe path, so it also catches launching a *different copy* of the binary (e.g. `dist\` versus `target\release\`).
  - **The `thread::spawn` + `run_on_main_thread` hop in `restore_visible_notes()` is load-bearing, not boilerplate.** The callback is delivered via `WM_COPYDATA` on the first instance's message loop, and `open_note_window()` can call `WebviewWindowBuilder::build()`. Building a window reentrantly during message dispatch is precisely the WebView2 deadlock documented above for `create_note`, so the same deferral is used to get a genuine event-loop tick instead of an inline call. `open_note_window()` is also the right primitive to reuse for the other half of that fix: it uses `.show()` and never `.set_focus()`.
  - **Trade-off accepted**: if the running instance ever wedges, relaunching now exits instead of giving you a working window, and recovery is killing the process in Task Manager.
