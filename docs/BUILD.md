# Release Build Internals

What `build-release.ps1` actually does and why a plain `npm run tauri build` is not enough for a build you intend to ship. See the main [README](../README.md) for day-to-day build commands.

## Quick reference

```powershell
.\build-release.ps1            # build + sign
.\build-release.ps1 -Deploy    # build + sign + swap into dist\ and relaunch
.\build-release.ps1 -SkipSign  # build only
```

Frontend changes need a full rebuild too -- in a release build, `src/*.html|css|js` get embedded into the compiled binary; editing them on disk has no effect until you rebuild (unlike `tauri dev`, which serves them live).

## What gets stripped and why

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

**Trade-off**: no `.pdb` is produced at all now, so a future crash dump resolves only to module+offset. That is exactly what the deadlock investigation in [`DEVELOPMENT_NOTES.md`](DEVELOPMENT_NOTES.md) worked from, so this costs nothing that was being relied on -- but if you ever need full symbols, build once with `strip = false` and without `/DEBUG:NONE`, and keep that `.pdb` locally rather than shipping it.
