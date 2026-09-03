use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub content: String,
    pub color: String,
    pub position: Position,
    pub size: Size,
    #[serde(default = "default_true")]
    pub wrap: bool,
    #[serde(default)]
    pub pinned: bool,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NoteFields {
    pub id: String,
    pub title: String,
    pub content: String,
    pub color: String,
    pub wrap: bool,
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn notes_dir(app: &AppHandle) -> PathBuf {
    let dir = app
        .path()
        .app_data_dir()
        .expect("app data dir unavailable")
        .join("notes");
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
    dir
}

fn note_path(app: &AppHandle, id: &str) -> PathBuf {
    notes_dir(app).join(format!("{id}.json"))
}

pub fn list_notes(app: &AppHandle) -> Vec<Note> {
    let dir = notes_dir(app);
    let mut notes = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(text) = fs::read_to_string(&path) {
                    if let Ok(note) = serde_json::from_str::<Note>(&text) {
                        notes.push(note);
                    }
                }
            }
        }
    }
    notes.sort_by_key(|n| n.created_at);
    notes
}

pub fn load_note(app: &AppHandle, id: &str) -> Result<Note, String> {
    let path = note_path(app, id);
    let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

pub fn save_note_to_disk(app: &AppHandle, note: &Note) {
    let mut note = note.clone();
    note.updated_at = now_millis();
    let path = note_path(app, &note.id);
    if let Ok(text) = serde_json::to_string_pretty(&note) {
        let _ = fs::write(path, text);
    }
}

pub fn update_note_fields(app: &AppHandle, fields: &NoteFields) -> Result<(), String> {
    let mut note = load_note(app, &fields.id)?;
    note.title = fields.title.clone();
    note.content = fields.content.clone();
    note.color = fields.color.clone();
    note.wrap = fields.wrap;
    save_note_to_disk(app, &note);
    Ok(())
}

pub fn update_note_pinned(app: &AppHandle, id: &str, pinned: bool) {
    if let Ok(mut note) = load_note(app, id) {
        note.pinned = pinned;
        save_note_to_disk(app, &note);
    }
}

pub fn update_note_geometry(app: &AppHandle, id: &str, position: Option<Position>, size: Option<Size>) {
    if let Ok(mut note) = load_note(app, id) {
        if let Some(p) = position {
            note.position = p;
        }
        if let Some(s) = size {
            note.size = s;
        }
        save_note_to_disk(app, &note);
    }
}

pub fn delete_note(app: &AppHandle, id: &str) -> Result<(), String> {
    let path = note_path(app, id);
    fs::remove_file(path).map_err(|e| e.to_string())
}

pub fn create_note_default(offset: f64) -> Note {
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_millis();
    Note {
        id,
        title: String::new(),
        content: String::new(),
        color: "#fff59d".to_string(),
        position: Position {
            x: 120.0 + offset,
            y: 120.0 + offset,
        },
        wrap: false,
        pinned: false,
        size: Size {
            width: 260.0,
            height: 260.0,
        },
        created_at: now,
        updated_at: now,
    }
}
