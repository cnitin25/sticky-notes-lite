mod notes;

use notes::{Note, NoteFields, Position, Size};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};

struct AppState {
    open_ids: Mutex<HashSet<String>>,
    next_offset: Mutex<i32>,
    quitting: AtomicBool,
}

const NOTE_PREFIX: &str = "note-";

fn window_label(id: &str) -> String {
    format!("{NOTE_PREFIX}{id}")
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

    if builder.build().is_ok() {
        let state = app.state::<AppState>();
        state.open_ids.lock().unwrap().insert(note.id.clone());
    }
}

fn spawn_new_note(app: &AppHandle) {
    let offset = {
        let state = app.state::<AppState>();
        let mut offset = state.next_offset.lock().unwrap();
        let current = *offset as f64;
        *offset = (*offset + 30) % 300;
        current
    };

    let note = notes::create_note_default(offset);
    notes::save_note_to_disk(app, &note);
    open_note_window(app, &note);
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
fn minimize_note(window: tauri::Window) -> Result<(), String> {
    window.hide().map_err(|e| e.to_string())
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
    {
        let state = app.state::<AppState>();
        state.open_ids.lock().unwrap().remove(&id);
    }
    let _ = window.close();
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState {
            open_ids: Mutex::new(HashSet::new()),
            next_offset: Mutex::new(0),
            quitting: AtomicBool::new(false),
        })
        .invoke_handler(tauri::generate_handler![
            load_note,
            save_note,
            create_note,
            minimize_note,
            set_pinned,
            delete_note
        ])
        .setup(|app| {
            let handle = app.handle().clone();

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
                        app.state::<AppState>()
                            .quitting
                            .store(true, Ordering::SeqCst);
                        app.exit(0);
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
            WindowEvent::CloseRequested { .. } => {
                let label = window.label().to_string();
                if let Some(id) = label.strip_prefix(NOTE_PREFIX) {
                    let state = window.app_handle().state::<AppState>();
                    state.open_ids.lock().unwrap().remove(id);
                }
            }
            WindowEvent::Moved(pos) => {
                let label = window.label().to_string();
                if let Some(id) = label.strip_prefix(NOTE_PREFIX) {
                    // WindowEvent delivers physical pixels, but the builder's
                    // .position()/.inner_size() take logical ones -- convert
                    // so a restored window lands where it was left, on any
                    // DPI-scaled display.
                    let scale = window.scale_factor().unwrap_or(1.0);
                    notes::update_note_geometry(
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
                    notes::update_note_geometry(
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
                }
            }
        });
}
