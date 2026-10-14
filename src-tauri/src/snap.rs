//! Magnetic docking between note windows while one is being dragged.
//!
//! Each note is its own OS window and its webview cannot see the others, so
//! this lives in the backend. It hooks `WM_MOVING`, which Windows sends from
//! inside its own drag loop with the proposed window rect -- adjusting that
//! rect in place is the only way to snap without the window fighting the
//! cursor. Repositioning from `WindowEvent::Moved` with `set_position()`
//! would arrive a frame late and jitter against the OS loop.
//!
//! On release the note takes its neighbour's height (docked left/right) or
//! width (docked above/below). Holding Alt while releasing skips that.
//!
//! The subclass procedure deliberately makes no Tauri calls: it runs inside
//! the OS modal move loop on the main thread, which is exactly where the
//! WebView2 deadlocks in `docs/DEVELOPMENT_NOTES.md` live. It only reads a
//! plain HWND registry and does Win32 rect arithmetic.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Mutex;

use tauri::WebviewWindow;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_MENU};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowRect, IsIconic, IsWindowVisible, PostMessageW, SetWindowPos, SWP_NOACTIVATE,
    SWP_NOZORDER, WM_APP, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_MOVING, WM_NCDESTROY,
};

/// How close (in 96-DPI pixels) an edge has to come before it is pulled in.
const SNAP_DISTANCE: i32 = 16;
/// Mirrors `.min_inner_size(180.0, 160.0)` in `open_note_window`.
const MIN_WIDTH: i32 = 180;
const MIN_HEIGHT: i32 = 160;

const SNAP_SUBCLASS_ID: usize = 0x534e_4150; // "SNAP"
/// Posted to itself on release, so the size match happens on a fresh
/// event-loop tick rather than inside the move loop's teardown.
const WM_APP_SNAP_RESIZE: u32 = WM_APP + 0x51;

/// Every note window's HWND, as `isize` (a raw `HWND` isn't `Send`).
static NOTES: Mutex<Vec<isize>> = Mutex::new(Vec::new());

/// Screen-space rectangle of a note's *visible* bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    fn width(&self) -> i32 {
        self.right - self.left
    }
    fn height(&self) -> i32 {
        self.bottom - self.top
    }
    fn offset(&self, dx: i32, dy: i32) -> Rect {
        Rect {
            left: self.left + dx,
            top: self.top + dy,
            right: self.right + dx,
            bottom: self.bottom + dy,
        }
    }
}

/// Which side of `other` the moving note docked onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Above,
    Below,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dock {
    pub other: Rect,
    pub side: Side,
}

/// Pull `moving` flush against the nearest edge of any of `others` within
/// `threshold`, keeping its size. A side dock needs the two to overlap
/// vertically and a top/bottom dock horizontally -- otherwise two notes far
/// apart on the other axis would "dock" across empty space. Once docked, the
/// perpendicular edge also aligns if it is within the threshold.
///
/// Ties on distance go to the note with the longer shared edge. Notes stacked
/// by an earlier dock share an edge exactly, so docking beside the stack is
/// always a tie; without this the first-opened note won however little of it
/// the moving note actually touched.
pub fn snap(moving: Rect, others: &[Rect], threshold: i32) -> (Rect, Option<Dock>) {
    // (distance, shared edge length, dock, dx, dy)
    let mut best: Option<(i32, i32, Dock, i32, i32)> = None;
    for &other in others {
        let v_overlap = moving.bottom.min(other.bottom) - moving.top.max(other.top);
        let h_overlap = moving.right.min(other.right) - moving.left.max(other.left);
        let candidates = [
            (v_overlap, Side::Right, other.right - moving.left, 0),
            (v_overlap, Side::Left, other.left - moving.right, 0),
            (h_overlap, Side::Below, 0, other.bottom - moving.top),
            (h_overlap, Side::Above, 0, other.top - moving.bottom),
        ];
        for (overlap, side, dx, dy) in candidates {
            let distance = dx.abs() + dy.abs();
            if overlap <= 0 || distance > threshold {
                continue;
            }
            let better = best.map_or(true, |(d, o, ..)| distance < d || (distance == d && overlap > o));
            if better {
                best = Some((distance, overlap, Dock { other, side }, dx, dy));
            }
        }
    }
    let Some((_, _, dock, dx, dy)) = best else {
        return (moving, None);
    };
    let docked = moving.offset(dx, dy);
    let other = dock.other;
    let (ax, ay) = match dock.side {
        Side::Left | Side::Right => (0, nearest_align(docked.top, docked.bottom, other.top, other.bottom, threshold)),
        Side::Above | Side::Below => (nearest_align(docked.left, docked.right, other.left, other.right, threshold), 0),
    };
    (docked.offset(ax, ay), Some(dock))
}

/// Where the note would be with no snapping: where it was at the first
/// `WM_MOVING`, plus how far the cursor has travelled since.
///
/// Snapping must start from this, never from the rect Windows proposes. The
/// drag loop builds each proposed rect from the *previous, already-snapped*
/// one plus a few pixels of mouse movement -- so snapping that pulled the note
/// back each time swallowed every step and a docked note could never be
/// dragged away again.
pub fn unsnapped(anchor: Rect, anchor_cursor: (i32, i32), cursor: (i32, i32)) -> Rect {
    anchor.offset(cursor.0 - anchor_cursor.0, cursor.1 - anchor_cursor.1)
}

/// Offset that lines up the start edges, else the end edges, if either pair is
/// within `threshold`; 0 otherwise.
fn nearest_align(start: i32, end: i32, other_start: i32, other_end: i32, threshold: i32) -> i32 {
    let d_start = other_start - start;
    let d_end = other_end - end;
    if d_start.abs() <= threshold && d_start.abs() <= d_end.abs() {
        d_start
    } else if d_end.abs() <= threshold {
        d_end
    } else {
        0
    }
}

/// The rect a docked note takes on release: beside a note it adopts that
/// note's top and height, above/below it adopts its left edge and width. The
/// docked edge itself stays put. Never shrinks below the window's minimum.
pub fn match_size(r: Rect, dock: Dock, min_w: i32, min_h: i32) -> Rect {
    let o = dock.other;
    match dock.side {
        Side::Left | Side::Right => Rect {
            top: o.top,
            bottom: o.top + o.height().max(min_h),
            ..r
        },
        Side::Above | Side::Below => Rect {
            left: o.left,
            right: o.left + o.width().max(min_w),
            ..r
        },
    }
}

fn from_win(r: RECT) -> Rect {
    Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
}

fn to_win(r: Rect) -> RECT {
    RECT { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
}

/// What the user sees, as opposed to `GetWindowRect`, which can include an
/// invisible resize border -- docking on the latter would leave a visible gap.
unsafe fn visible_bounds(hwnd: HWND) -> Option<Rect> {
    let mut r = RECT::default();
    let size = std::mem::size_of::<RECT>() as u32;
    if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut _, size).is_ok() {
        return Some(from_win(r));
    }
    GetWindowRect(hwnd, &mut r).ok().map(|_| from_win(r))
}

/// Visible bounds minus window rect, per edge, for the window being dragged.
#[derive(Clone, Copy, Default)]
struct Insets {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl Insets {
    fn to_visible(&self, r: Rect) -> Rect {
        Rect {
            left: r.left + self.left,
            top: r.top + self.top,
            right: r.right + self.right,
            bottom: r.bottom + self.bottom,
        }
    }
    fn to_window(&self, r: Rect) -> Rect {
        Rect {
            left: r.left - self.left,
            top: r.top - self.top,
            right: r.right - self.right,
            bottom: r.bottom - self.bottom,
        }
    }
}

struct Drag {
    hwnd: isize,
    others: Vec<Rect>,
    insets: Insets,
    threshold: i32,
    dock: Option<Dock>,
    /// Visible rect and cursor at the first `WM_MOVING`; see `unsnapped()`.
    anchor: Option<(Rect, (i32, i32))>,
}

thread_local! {
    /// The one drag in progress. Windows runs a single modal move loop at a
    /// time on this thread, so one slot is enough.
    static DRAG: RefCell<Option<Drag>> = const { RefCell::new(None) };
    /// Size-matched window rects waiting for their `WM_APP_SNAP_RESIZE`.
    static PENDING: RefCell<HashMap<isize, Rect>> = RefCell::new(HashMap::new());
}

fn scaled(px: i32, dpi: u32) -> i32 {
    px * dpi as i32 / 96
}

/// Start docking `window` to its neighbours. Called once, right after the
/// window is built.
pub fn attach(window: &WebviewWindow) {
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    unsafe {
        if SetWindowSubclass(hwnd, Some(subclass_proc), SNAP_SUBCLASS_ID, 0).as_bool() {
            NOTES.lock().unwrap_or_else(|e| e.into_inner()).push(hwnd.0 as isize);
        }
    }
}

unsafe fn begin_drag(hwnd: HWND) {
    let me = hwnd.0 as isize;
    let others: Vec<Rect> = NOTES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|&&h| h != me)
        .map(|&h| HWND(h as _))
        // Minimized notes are hidden windows; docking to one would look like
        // snapping to empty space.
        .filter(|&h| IsWindowVisible(h).as_bool() && !IsIconic(h).as_bool())
        .filter_map(|h| visible_bounds(h))
        .collect();

    let mut wr = RECT::default();
    let insets = match (GetWindowRect(hwnd, &mut wr), visible_bounds(hwnd)) {
        (Ok(()), Some(v)) => Insets {
            left: v.left - wr.left,
            top: v.top - wr.top,
            right: v.right - wr.right,
            bottom: v.bottom - wr.bottom,
        },
        _ => Insets::default(),
    };
    let threshold = scaled(SNAP_DISTANCE, GetDpiForWindow(hwnd).max(96));
    DRAG.with(|d| {
        *d.borrow_mut() = Some(Drag { hwnd: me, others, insets, threshold, dock: None, anchor: None });
    });
}

unsafe fn on_moving(hwnd: HWND, proposed: *mut RECT) {
    if proposed.is_null() {
        return;
    }
    DRAG.with(|d| {
        let mut d = d.borrow_mut();
        let Some(drag) = d.as_mut().filter(|drag| drag.hwnd == hwnd.0 as isize) else {
            return;
        };
        if drag.others.is_empty() {
            return;
        }
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_err() {
            return;
        }
        let cursor = (pt.x, pt.y);
        // The first proposal is still untouched by us, so it anchors the drag.
        let (anchor, anchor_cursor) = *drag
            .anchor
            .get_or_insert_with(|| (drag.insets.to_visible(from_win(*proposed)), cursor));
        let free = unsnapped(anchor, anchor_cursor, cursor);
        let (snapped, dock) = snap(free, &drag.others, drag.threshold);
        drag.dock = dock;
        *proposed = to_win(drag.insets.to_window(snapped));
    });
}

unsafe fn end_drag(hwnd: HWND) {
    let Some(drag) = DRAG.with(|d| d.borrow_mut().take()) else {
        return;
    };
    let me = hwnd.0 as isize;
    if drag.hwnd != me {
        return;
    }
    let Some(dock) = drag.dock else {
        return;
    };
    // Alt held on release: keep the snapped position, skip the size match.
    if GetKeyState(VK_MENU.0 as i32) < 0 {
        return;
    }
    let Some(visible) = visible_bounds(hwnd) else {
        return;
    };
    let dpi = GetDpiForWindow(hwnd).max(96);
    let matched = match_size(visible, dock, scaled(MIN_WIDTH, dpi), scaled(MIN_HEIGHT, dpi));
    if matched == visible {
        return;
    }
    PENDING.with(|p| p.borrow_mut().insert(me, drag.insets.to_window(matched)));
    let _ = PostMessageW(Some(hwnd), WM_APP_SNAP_RESIZE, WPARAM(0), LPARAM(0));
}

unsafe fn apply_pending(hwnd: HWND) {
    let Some(r) = PENDING.with(|p| p.borrow_mut().remove(&(hwnd.0 as isize))) else {
        return;
    };
    // No activation and no z-order change: forcing focus between WebView2
    // windows is the deadlock trigger documented for `.set_focus()`.
    let _ = SetWindowPos(
        hwnd,
        None,
        r.left,
        r.top,
        r.width(),
        r.height(),
        SWP_NOZORDER | SWP_NOACTIVATE,
    );
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    match msg {
        WM_ENTERSIZEMOVE => begin_drag(hwnd),
        WM_MOVING => {
            on_moving(hwnd, lparam.0 as *mut RECT);
            let _ = DefSubclassProc(hwnd, msg, wparam, lparam);
            return LRESULT(1);
        }
        WM_EXITSIZEMOVE => end_drag(hwnd),
        WM_APP_SNAP_RESIZE => {
            apply_pending(hwnd);
            return LRESULT(0);
        }
        WM_NCDESTROY => {
            let me = hwnd.0 as isize;
            NOTES.lock().unwrap_or_else(|e| e.into_inner()).retain(|&h| h != me);
            PENDING.with(|p| p.borrow_mut().remove(&me));
            let _ = RemoveWindowSubclass(hwnd, Some(subclass_proc), SNAP_SUBCLASS_ID);
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(left: i32, top: i32, w: i32, h: i32) -> Rect {
        Rect { left, top, right: left + w, bottom: top + h }
    }

    const T: i32 = 16;

    #[test]
    fn docks_to_the_right_and_aligns_top() {
        let other = r(100, 100, 200, 200);
        let (out, dock) = snap(r(310, 108, 150, 150), &[other], T);
        assert_eq!(out, r(300, 100, 150, 150));
        assert_eq!(dock.unwrap().side, Side::Right);
    }

    #[test]
    fn docks_on_each_side() {
        let other = r(100, 100, 200, 200);
        assert_eq!(snap(r(-55, 150, 150, 150), &[other], T).1.unwrap().side, Side::Left);
        assert_eq!(snap(r(150, 305, 150, 150), &[other], T).1.unwrap().side, Side::Below);
        let (out, dock) = snap(r(150, -45, 150, 150), &[other], T);
        assert_eq!(dock.unwrap().side, Side::Above);
        assert_eq!(out.bottom, 100);
    }

    #[test]
    fn needs_overlap_on_the_other_axis() {
        // Right edges touch, but the moving note is entirely below the other.
        let other = r(100, 100, 200, 200);
        let (out, dock) = snap(r(305, 500, 150, 150), &[other], T);
        assert_eq!(out, r(305, 500, 150, 150));
        assert!(dock.is_none());
    }

    #[test]
    fn outside_threshold_does_not_snap() {
        let other = r(100, 100, 200, 200);
        let moving = r(300 + T + 1, 120, 150, 150);
        assert_eq!(snap(moving, &[other], T), (moving, None));
    }

    #[test]
    fn nearest_candidate_wins() {
        let far = r(0, 100, 290, 200); // right edge at 290, 12px away
        let near = r(500, 100, 200, 200); // left edge at 500, 3px away
        let (out, dock) = snap(r(302, 100, 195, 150), &[far, near], T);
        assert_eq!(dock.unwrap().other, near);
        assert_eq!(out.right, 500);
    }

    /// Docking beside a stack: both stacked notes share the left edge, so the
    /// distance ties, and the one the moving note mostly touches must win --
    /// regardless of which was opened (listed) first.
    #[test]
    fn tie_goes_to_the_longer_shared_edge() {
        let upper = r(400, 0, 300, 230);
        let lower = r(400, 230, 300, 360);
        let moving = r(95, 215, 300, 250); // 15px of the upper, 235px of the lower
        for others in [[upper, lower], [lower, upper]] {
            let (_, dock) = snap(moving, &others, T);
            assert_eq!(dock.unwrap().other, lower);
        }
    }

    #[test]
    fn perpendicular_edge_left_alone_when_far() {
        let other = r(100, 100, 200, 200);
        let (out, _) = snap(r(305, 150, 150, 100), &[other], T);
        assert_eq!(out, r(300, 150, 150, 100));
    }

    /// Regression: replays the OS drag loop, which builds each proposed rect
    /// from the previous snapped one plus the mouse delta. Dragging a docked
    /// note away 2px at a time must still escape once the cursor has moved
    /// past the threshold.
    #[test]
    fn docked_note_can_be_dragged_away_in_small_steps() {
        let other = r(100, 100, 200, 200);
        let start = r(300, 100, 150, 150); // docked to the right
        let anchor_cursor = (350, 120);
        let mut shown = start;
        let mut cursor = anchor_cursor;
        for _ in 0..30 {
            cursor.0 += 2;
            let free = unsnapped(start, anchor_cursor, cursor);
            shown = snap(free, &[other], T).0;
        }
        assert_eq!(shown, r(360, 100, 150, 150));

        // The old behaviour, for contrast: snapping the OS proposal directly
        // pulls it back every step, so it never leaves.
        let mut stuck = start;
        for _ in 0..30 {
            stuck = snap(stuck.offset(2, 0), &[other], T).0;
        }
        assert_eq!(stuck, start);
    }

    #[test]
    fn match_size_beside_takes_height_and_top() {
        let other = r(100, 100, 200, 300);
        let dock = Dock { other, side: Side::Right };
        assert_eq!(match_size(r(300, 150, 150, 150), dock, 180, 160), r(300, 100, 150, 300));
    }

    #[test]
    fn match_size_below_takes_width_and_left() {
        let other = r(100, 100, 250, 200);
        let dock = Dock { other, side: Side::Below };
        assert_eq!(match_size(r(130, 300, 180, 160), dock, 180, 160), r(100, 300, 250, 160));
    }

    #[test]
    fn match_size_respects_minimum() {
        let other = r(100, 100, 120, 100);
        let side = Dock { other, side: Side::Left };
        assert_eq!(match_size(r(-80, 100, 180, 200), side, 180, 160).height(), 160);
        let below = Dock { other, side: Side::Below };
        assert_eq!(match_size(r(100, 200, 200, 200), below, 180, 160).width(), 180);
    }
}
