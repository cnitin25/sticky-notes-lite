# Sticky Notes Lite

A lightweight, fully offline sticky-notes app for Windows, built with Tauri v2 (Rust backend + vanilla HTML/CSS/JS frontend, no bundler). Each note is its own draggable, resizable OS window; there's no visible main window — the app lives entirely in the system tray. Notes persist as individual JSON files, auto-saved as you type.

## Features

- Separate OS windows per note — drag/resize independently, anywhere on screen (including across monitors)
- Per-note editable title, background color (fixed 8-color palette), and text-wrap toggle (default off)
- One-click **copy** (copies note content only, not the title) and **paste** (replaces content entirely)
- **Minimize**: hides the note window; bring it back via the tray's "Show All Notes"
- **Pin**: keeps a note always on top of every other window (not just other notes) — persisted, so a pinned note reopens pinned
- **New note** button on every note's toolbar (in addition to the tray's "New Note") — a fresh note isn't focused automatically (click into it before typing); this is deliberate, see "Notable bugs fixed" below
- Delete with an in-page confirmation dialog (not the browser's native `confirm()`, which clips on small windows)
- Auto-save (debounced ~500ms) to `%APPDATA%\com.stickynoteslite.desktop\notes\<uuid>.json`
- Position/size auto-saved on drag/resize, restored on next launch — with a safety clamp so a note can never reopen fully off-screen
- No taskbar icon (tray-only); system tray menu: **New Note**, **Show All Notes**, **Quit**
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
npm run tauri build
```

Produces `src-tauri\target\release\sticky-notes-lite.exe` (portable) plus MSI/NSIS installers under `src-tauri\target\release\bundle\`. `target\` is disposable build cache (can grow to several GB) — safe to delete, `npm run tauri build` regenerates it from scratch (a few minutes) whenever needed.

**Frontend changes need a full rebuild too** — in a release build, `src/*.html|css|js` get embedded into the compiled binary; editing them on disk has no effect until you rebuild (unlike `tauri dev`, which serves them live).

## Code signing (this machine has Smart App Control enabled)

Windows 11 Smart App Control (SAC) blocks unsigned executables outright — no "Run anyway" override, unlike classic SmartScreen. Every new build needs re-signing, since each build is a new file hash.

### Current certificate (already created and trusted on this machine)

- Subject: `CN=Sticky Notes Lite (Local)`
- Thumbprint: `2FD74642CE56E472D2CB807D396B77DA651FEF41`
- Store: `Cert:\CurrentUser\My` (private key), trusted via `LocalMachine\Root` + `LocalMachine\TrustedPublisher`
- Valid until 2036

Sign every new build with:

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

## Data location

Notes live in `%APPDATA%\com.stickynoteslite.desktop\notes\<uuid>.json` — independent of where the `.exe` runs from. Moving/renaming/copying the exe never affects existing notes; only changing `identifier` in `src-tauri\tauri.conf.json` would (don't).

Note JSON schema: `{ id, title, content, color, position: {x,y}, size: {width,height}, wrap, created_at, updated_at }` — all in logical (DPI-independent) pixels for position/size.

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
