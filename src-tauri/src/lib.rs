mod notes;

use notes::{Note, NoteFields, Position, Size};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};

/// Coalescing window for drag/resize writes. `Moved`/`Resized` arrive roughly
/// once per frame while a window is being dragged, so writing on each one meant
/// a full read-parse-serialize-write of the note's JSON per frame, on the main
/// thread that is also pumping WebView2. Geometry is now accumulated in memory
/// and written at most this often, from a background thread.
const GEOMETRY_FLUSH_MS: u64 = 400;

/// How long a note window waits for its webview to save and close itself before
/// it is closed anyway. Only reached if the frontend is wedged -- a note that
/// refuses to close would be worse than losing the last few keystrokes.
const CLOSE_WATCHDOG_MS: u64 = 800;

/// How long to give every note's webview to write out unsaved text after the
/// tray's Quit asks it to, before the process exits.
const QUIT_FLUSH_GRACE_MS: u64 = 250;

#[derive(Default)]
struct PendingGeometry {
    position: Option<Position>,
    size: Option<Size>,
}

struct AppState {
    next_offset: Mutex<i32>,
    quitting: AtomicBool,
    /// Geometry not yet written to disk, keyed by note id.
    pending_geometry: Mutex<HashMap<String, PendingGeometry>>,
    /// Window labels whose next `CloseRequested` should be allowed through --
    /// set once the webview has flushed its unsaved text.
    close_allowed: Mutex<HashSet<String>>,
}

const NOTE_PREFIX: &str = "note-";

fn window_label(id: &str) -> String {
    format!("{NOTE_PREFIX}{id}")
}

/// A panic elsewhere while holding one of these locks would otherwise poison it
/// and break every later save; the state they guard is plain data, so carrying
/// on with it is correct here.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn queue_geometry(app: &AppHandle, id: &str, position: Option<Position>, size: Option<Size>) {
    let state = app.state::<AppState>();
    let mut pending = lock(&state.pending_geometry);
    let entry = pending.entry(id.to_string()).or_default();
    if position.is_some() {
        entry.position = position;
    }
    if size.is_some() {
        entry.size = size;
    }
}

fn flush_geometry(app: &AppHandle) {
    let drained: Vec<(String, PendingGeometry)> = {
        let state = app.state::<AppState>();
        let mut pending = lock(&state.pending_geometry);
        if pending.is_empty() {
            return;
        }
        pending.drain().collect()
    };
    for (id, geometry) in drained {
        notes::update_note_geometry(app, &id, geometry.position, geometry.size);
    }
}

/// Ensure a note's saved position keeps at least its title bar on some
/// connected monitor. Guards against stale positions from a since-removed
/// monitor, a resolution change, or a physical/logical pixel mismatch.
fn clamp_to_visible_area(app: &AppHandle, pos: Position) -> Position {
    const MIN_VISIBLE: f64 = 60.0;
    if let Ok(monitors) = app.available_monitors() {
        let on_screen = monitors.iter().any(|m| {
            let scale = m.scale_factor();
            let mp = m.position();
            let ms = m.size();
            let mx0 = mp.x as f64 / scale;
            let my0 = mp.y as f64 / scale;
            let mx1 = mx0 + ms.width as f64 / scale;
            let my1 = my0 + ms.height as f64 / scale;
            pos.x + MIN_VISIBLE > mx0 && pos.x < mx1 && pos.y + MIN_VISIBLE > my0 && pos.y < my1
        });
        if !on_screen {
            if let Ok(Some(primary)) = app.primary_monitor() {
                let scale = primary.scale_factor();
                let pp = primary.position();
                return Position {
                    x: pp.x as f64 / scale + 120.0,
                    y: pp.y as f64 / scale + 120.0,
                };
            }
            return Position { x: 120.0, y: 120.0 };
        }
    }
    pos
}

fn open_note_window(app: &AppHandle, note: &Note) {
    let label = window_label(&note.id);
    if let Some(existing) = app.get_webview_window(&label) {
        // Already open (possibly minimized/hidden) -- bring it back instead of no-op.
        // Deliberately not calling .set_focus() here: forcing focus on multiple
        // WebView2-hosted windows back-to-back in a tight loop (as "Show All
        // Notes" does) has been observed to deadlock -- a memory dump of a
        // hang showed our main thread and a WebView2-internal compositor
        // thread each blocked in their own win32u.dll syscall, waiting on
        // each other. .show() alone is enough to bring a hidden note back.
        let _ = existing.show();
        return;
    }
    let url = format!("note.html?id={}", note.id);
    let title = if note.title.trim().is_empty() {
        "Note".to_string()
    } else {
        note.title.clone()
    };
    let position = clamp_to_visible_area(app, note.position.clone());
    let mut builder = WebviewWindowBuilder::new(app, label, WebviewUrl::App(url.into()))
        .title(title)
        .inner_size(note.size.width, note.size.height)
        .position(position.x, position.y)
        .min_inner_size(180.0, 160.0)
        .decorations(false)
        .resizable(true)
        .transparent(true)
        .shadow(false)
        .skip_taskbar(true)
        // Don't let native window creation auto-steal focus. Doing so
        // synchronously as part of build() -- while some other activation
        // (another window's own creation, or the tray icon's own focus
        // requirements) is concurrently in flight -- is what's been
        // deadlocking the app (confirmed via memory dump: our main thread
        // and a WebView2-internal thread each stuck in win32u.dll, waiting
        // on each other). Costs one extra click to start typing in a new
        // note; that's a small price for eliminating this deadlock class.
        .focused(false);
    // Only touch always-on-top for a pinned note -- calling it unconditionally
    // (even with `false`, which is already the default state for a brand new
    // window) while another window is genuinely topmost has been observed to
    // hang the whole app's main thread on this system.
    if note.pinned {
        builder = builder.always_on_top(true);
    }

    if let Err(e) = builder.build() {
        eprintln!("could not open note {}: {e}", note.id);
    }
}

fn spawn_new_note(app: &AppHandle) {
    let offset = {
        let state = app.state::<AppState>();
        let mut offset = lock(&state.next_offset);
        let current = *offset as f64;
        *offset = (*offset + 30) % 300;
        current
    };

    let note = notes::create_note_default(offset);
    if let Err(e) = notes::save_note_to_disk(app, &note) {
        // Opening a window for a note that isn't on disk would hand the user an
        // editor whose every save fails, so stop here instead.
        eprintln!("could not create note: {e}");
        return;
    }
    open_note_window(app, &note);
}

/// Mark a window as cleared to close, so the `CloseRequested` handler lets the
/// next attempt through instead of asking the webview to flush again.
fn allow_close(app: &AppHandle, label: &str) {
    lock(&app.state::<AppState>().close_allowed).insert(label.to_string());
}

#[tauri::command]
fn load_note(app: AppHandle, id: String) -> Result<Note, String> {
    notes::load_note(&app, &id)
}

#[tauri::command]
fn save_note(app: AppHandle, window: tauri::Window, fields: NoteFields) -> Result<(), String> {
    notes::update_note_fields(&app, &fields)?;
    let title = if fields.title.trim().is_empty() {
        "Note".to_string()
    } else {
        fields.title.clone()
    };
    let _ = window.set_title(&title);
    Ok(())
}

#[tauri::command]
fn create_note(app: AppHandle) -> Result<(), String> {
    // Command handlers for this backend already run ON the main thread, so
    // AppHandle::run_on_main_thread's "already on main thread" fast path
    // calls the closure immediately, inline, synchronously -- it does NOT
    // defer to the next event-loop tick the way its name suggests. Building
    // a new window reentrantly like that, in the middle of dispatching the
    // IPC message that invoked this command, is what's been deadlocking
    // the app against WebView2's own internal message pumping (confirmed
    // via memory dump: our thread and a WebView2-internal thread each stuck
    // in win32u.dll, waiting on each other).
    //
    // Forcing a genuine deferral: calling run_on_main_thread from a thread
    // that ISN'T the main one takes its other branch, which posts a real
    // event to tao's event loop queue instead -- processed on the next
    // iteration, not reentrantly.
    std::thread::spawn(move || {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || spawn_new_note(&handle));
    });
    Ok(())
}

#[tauri::command]
fn minimize_note(app: AppHandle, window: tauri::Window) -> Result<(), String> {
    // A hidden window stops emitting Moved/Resized, so settle its geometry now.
    flush_geometry(&app);
    window.hide().map_err(|e| e.to_string())
}

/// Called by a note's webview once it has written out any unsaved text in
/// response to `save-and-close`. See the `CloseRequested` handler.
#[tauri::command]
fn close_note(app: AppHandle, window: tauri::Window) -> Result<(), String> {
    allow_close(&app, window.label());
    window.close().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_pinned(app: AppHandle, window: tauri::Window, id: String, pinned: bool) -> Result<(), String> {
    window.set_always_on_top(pinned).map_err(|e| e.to_string())?;
    notes::update_note_pinned(&app, &id, pinned);
    Ok(())
}

#[tauri::command]
fn delete_note(app: AppHandle, window: tauri::Window, id: String) -> Result<(), String> {
    notes::delete_note(&app, &id)?;
    // Drop any geometry still queued for a note that no longer exists, and skip
    // the save-and-close handshake -- there is nothing left to save into.
    lock(&app.state::<AppState>().pending_geometry).remove(&id);
    allow_close(&app, window.label());
    let _ = window.close();
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState {
            next_offset: Mutex::new(0),
            quitting: AtomicBool::new(false),
            pending_geometry: Mutex::new(HashMap::new()),
            close_allowed: Mutex::new(HashSet::new()),
        })
        .invoke_handler(tauri::generate_handler![
            load_note,
            save_note,
            create_note,
            minimize_note,
            close_note,
            set_pinned,
            delete_note
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // Drain queued drag/resize geometry off the main thread.
            let geometry_handle = handle.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_millis(GEOMETRY_FLUSH_MS));
                flush_geometry(&geometry_handle);
            });

            let new_note_item = MenuItem::with_id(app, "new-note", "New Note", true, None::<&str>)?;
            let show_all_item =
                MenuItem::with_id(app, "show-all", "Show All Notes", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&new_note_item, &show_all_item, &quit_item])?;

            let mut tray = TrayIconBuilder::new()
                .tooltip("Sticky Notes Lite")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    // Genuinely deferred via a throwaway thread -- see the
                    // comment on the `create_note` command for why calling
                    // run_on_main_thread directly here isn't enough.
                    "new-note" => {
                        let app = app.clone();
                        std::thread::spawn(move || {
                            let handle = app.clone();
                            let _ = app.run_on_main_thread(move || spawn_new_note(&handle));
                        });
                    }
                    "show-all" => {
                        let app = app.clone();
                        std::thread::spawn(move || {
                            let handle = app.clone();
                            let _ = app.run_on_main_thread(move || {
                                for note in notes::list_notes(&handle) {
                                    open_note_window(&handle, &note);
                                }
                            });
                        });
                    }
                    "quit" => {
                        // Exiting straight away would discard anything typed
                        // inside the frontend's save debounce, so ask every
                        // note to write out first and give the IPC round trip
                        // a moment to land. Done off the main thread so the
                        // wait doesn't block the event loop those saves
                        // travel over.
                        let app = app.clone();
                        std::thread::spawn(move || {
                            let _ = app.emit("flush-save", ());
                            std::thread::sleep(Duration::from_millis(QUIT_FLUSH_GRACE_MS));
                            flush_geometry(&app);
                            app.state::<AppState>().quitting.store(true, Ordering::SeqCst);
                            let handle = app.clone();
                            let _ = app.run_on_main_thread(move || handle.exit(0));
                        });
                    }
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            let existing = notes::list_notes(&handle);
            if existing.is_empty() {
                spawn_new_note(&handle);
            } else {
                for note in existing {
                    open_note_window(&handle, &note);
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                let label = window.label().to_string();
                if !label.starts_with(NOTE_PREFIX) {
                    return;
                }
                let app = window.app_handle().clone();
                // Settle this note's position/size while its window still exists.
                flush_geometry(&app);

                let cleared = lock(&app.state::<AppState>().close_allowed).remove(&label);
                if cleared || app.state::<AppState>().quitting.load(Ordering::SeqCst) {
                    return;
                }

                // The webview may hold up to one debounce window of typing that
                // has never reached disk, and closing destroys it. Hold the
                // close, ask the note to save, and let it close itself.
                api.prevent_close();
                let _ = app.emit_to(label.as_str(), "save-and-close", ());

                // ...but never trap a note open if its webview doesn't answer.
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(CLOSE_WATCHDOG_MS));
                    if let Some(win) = app.get_webview_window(&label) {
                        allow_close(&app, &label);
                        let _ = win.close();
                    }
                });
            }
            WindowEvent::Moved(pos) => {
                let label = window.label().to_string();
                if let Some(id) = label.strip_prefix(NOTE_PREFIX) {
                    // WindowEvent delivers physical pixels, but the builder's
                    // .position()/.inner_size() take logical ones -- convert
                    // so a restored window lands where it was left, on any
                    // DPI-scaled display.
                    let scale = window.scale_factor().unwrap_or(1.0);
                    queue_geometry(
                        window.app_handle(),
                        id,
                        Some(Position {
                            x: pos.x as f64 / scale,
                            y: pos.y as f64 / scale,
                        }),
                        None,
                    );
                }
            }
            WindowEvent::Resized(size) => {
                let label = window.label().to_string();
                if let Some(id) = label.strip_prefix(NOTE_PREFIX) {
                    let scale = window.scale_factor().unwrap_or(1.0);
                    queue_geometry(
                        window.app_handle(),
                        id,
                        None,
                        Some(Size {
                            width: size.width as f64 / scale,
                            height: size.height as f64 / scale,
                        }),
                    );
                }
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                // Keep running in the tray even when every note window is closed;
                // only an explicit tray "Quit" (which sets `quitting` first) is
                // allowed through.
                let quitting = app_handle
                    .state::<AppState>()
                    .quitting
                    .load(Ordering::SeqCst);
                if !quitting {
                    api.prevent_exit();
                } else {
                    // Last chance to persist geometry queued but not yet drained.
                    flush_geometry(app_handle);
                }
            }
        });
}
