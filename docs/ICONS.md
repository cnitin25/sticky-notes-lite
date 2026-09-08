# App Icon Workflow

How the icon under `src-tauri/icons/` is generated, and two Windows-specific gotchas that look like build failures but aren't. See the main [README](../README.md) for everything else.

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
