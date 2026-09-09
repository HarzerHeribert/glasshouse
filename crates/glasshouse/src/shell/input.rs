//! Translation from Glasshouse's terminal events to a focused child terminal.

use std::collections::VecDeque;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

use crate::pty::TerminalSize;
use crate::session::SessionRuntime;
use crate::tui::{MouseCapture, Screen};

use super::hotspot::{self, Hotspot};
use super::state::{self, Mode, ShellState};
use super::view;

const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

pub(super) fn route_mouse(
    event: MouseEvent,
    outer: TerminalSize,
    state: &ShellState,
    hotspots: &[Hotspot],
    pending: &mut VecDeque<KeyEvent>,
    live: &mut SessionRuntime,
) {
    // Terminals reserve Shift-drag for their own native text selection. If
    // one still reports the modified event, leave it untouched rather than
    // turning it into either a Glasshouse action or child input.
    if event.modifiers.contains(KeyModifiers::SHIFT) {
        return;
    }
    if matches!(event.kind, MouseEventKind::Down(MouseButton::Left))
        && let Some(spot) = hotspot::hit(hotspots, event.column, event.row)
    {
        // A click on Glasshouse chrome gives that chrome the keyboard before
        // replaying its advertised keys. This keeps a tab click from becoming
        // a literal Tab in the child while preserving the existing key path.
        if state.mode() == Mode::Session {
            pending.push_back(state::focus_chord_key());
        }
        pending.extend(spot.keys().iter().copied());
        return;
    }

    if state.overlay().is_some() {
        return;
    }
    let viewport = view::viewport_slot(Rect::new(0, 0, outer.cols, outer.rows), state.chrome());
    let inside = event.column >= viewport.x
        && event.column < viewport.right()
        && event.row >= viewport.y
        && event.row < viewport.bottom();

    // A first click from control mode focuses the viewport only. It changes
    // the chrome and child size, so forwarding it with old coordinates would
    // be wrong. Once focused, reports are translated only when requested.
    if inside && state.mode() == Mode::Control {
        if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
            pending.push_back(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        }
    } else if state.mode() == Mode::Session
        && let Some(modes) = live.focused_input_modes()
        && let Some(bytes) = mouse(event, viewport, modes.mouse_mode, modes.mouse_encoding)
        && let Err(err) = live.write_to_focused(&bytes)
    {
        tracing::warn!(%err, "could not forward mouse input to the focused session");
    }
}

pub(super) fn route_paste(text: &str, state: &ShellState, live: &mut SessionRuntime) {
    if state.mode() == Mode::Session
        && let Some(modes) = live.focused_input_modes()
    {
        let bytes = paste(text, modes.bracketed_paste);
        if let Err(err) = live.write_to_focused(&bytes) {
            tracing::warn!(%err, "could not paste into the focused session");
        }
    }
}

pub(super) fn sync_mouse_capture(screen: &mut Screen, state: &ShellState, live: &SessionRuntime) {
    let capture = if state.mode() == Mode::Session {
        live.focused_input_modes()
            .map_or(MouseCapture::PressRelease, |modes| {
                host_capture(modes.mouse_mode)
            })
    } else {
        MouseCapture::PressRelease
    };
    screen.set_mouse_capture(capture);
}

pub(super) fn host_capture(mode: MouseProtocolMode) -> MouseCapture {
    match mode {
        MouseProtocolMode::ButtonMotion => MouseCapture::ButtonMotion,
        MouseProtocolMode::AnyMotion => MouseCapture::AnyMotion,
        MouseProtocolMode::None | MouseProtocolMode::Press | MouseProtocolMode::PressRelease => {
            MouseCapture::PressRelease
        }
    }
}

pub(super) fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    if !bracketed {
        return text.as_bytes().to_vec();
    }
    let mut bytes = Vec::with_capacity(PASTE_START.len() + text.len() + PASTE_END.len());
    bytes.extend_from_slice(PASTE_START);
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(PASTE_END);
    bytes
}

/// Encode one mouse report for the focused child.
///
/// `viewport` is the exact rectangle drawn in the current frame. Crossterm
/// reports zero-based coordinates for the outer Glasshouse terminal; xterm
/// protocols report one-based coordinates for the child's terminal.
pub(super) fn mouse(
    event: MouseEvent,
    viewport: Rect,
    mode: MouseProtocolMode,
    encoding: MouseProtocolEncoding,
) -> Option<Vec<u8>> {
    if !inside(viewport, event.column, event.row) || !mode_reports(mode, event.kind) {
        return None;
    }

    let x = event.column.checked_sub(viewport.x)?.checked_add(1)?;
    let y = event.row.checked_sub(viewport.y)?.checked_add(1)?;
    let (mut button, release) = button_code(event.kind, encoding)?;
    if event.modifiers.contains(KeyModifiers::SHIFT) {
        button |= 4;
    }
    if event.modifiers.contains(KeyModifiers::ALT) {
        button |= 8;
    }
    if event.modifiers.contains(KeyModifiers::CONTROL) {
        button |= 16;
    }

    match encoding {
        MouseProtocolEncoding::Sgr => {
            Some(format!("\x1b[<{button};{x};{y}{}", if release { 'm' } else { 'M' }).into_bytes())
        }
        MouseProtocolEncoding::Default => {
            // The legacy protocol stores each value as one byte after adding
            // 32, so coordinates beyond 223 cannot be represented honestly.
            if x > 223 || y > 223 {
                return None;
            }
            Some(vec![
                0x1b,
                b'[',
                b'M',
                u8::try_from(button + 32).ok()?,
                u8::try_from(x + 32).ok()?,
                u8::try_from(y + 32).ok()?,
            ])
        }
        MouseProtocolEncoding::Utf8 => {
            // Xterm's 1005 protocol uses at most two UTF-8 bytes per value.
            // With the 32 offset, 2015 is the largest representable child
            // coordinate; a three-byte value is not a valid mouse report.
            if x > 2015 || y > 2015 {
                return None;
            }
            let mut bytes = b"\x1b[M".to_vec();
            push_codepoint(&mut bytes, u32::from(button) + 32)?;
            push_codepoint(&mut bytes, u32::from(x) + 32)?;
            push_codepoint(&mut bytes, u32::from(y) + 32)?;
            Some(bytes)
        }
    }
}

fn inside(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x && column < area.right() && row >= area.y && row < area.bottom()
}

fn mode_reports(mode: MouseProtocolMode, kind: MouseEventKind) -> bool {
    match mode {
        MouseProtocolMode::None => false,
        MouseProtocolMode::Press => matches!(
            kind,
            MouseEventKind::Down(_)
                | MouseEventKind::ScrollDown
                | MouseEventKind::ScrollUp
                | MouseEventKind::ScrollLeft
                | MouseEventKind::ScrollRight
        ),
        MouseProtocolMode::PressRelease => {
            !matches!(kind, MouseEventKind::Drag(_) | MouseEventKind::Moved)
        }
        MouseProtocolMode::ButtonMotion => !matches!(kind, MouseEventKind::Moved),
        MouseProtocolMode::AnyMotion => true,
    }
}

fn button_code(kind: MouseEventKind, encoding: MouseProtocolEncoding) -> Option<(u16, bool)> {
    let button = |button| match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    Some(match kind {
        MouseEventKind::Down(value) => (button(value), false),
        MouseEventKind::Up(value) if encoding == MouseProtocolEncoding::Sgr => {
            (button(value), true)
        }
        MouseEventKind::Up(_) => (3, false),
        MouseEventKind::Drag(value) => (32 | button(value), false),
        MouseEventKind::Moved => (35, false),
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        MouseEventKind::ScrollLeft => (66, false),
        MouseEventKind::ScrollRight => (67, false),
    })
}

fn push_codepoint(bytes: &mut Vec<u8>, value: u32) -> Option<()> {
    let value = char::from_u32(value)?;
    let mut encoded = [0; 4];
    bytes.extend_from_slice(value.encode_utf8(&mut encoded).as_bytes());
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn bracketed_paste_preserves_multiline_unicode_and_control_bytes() {
        let text = "one\n雪\x1b[31m\rthree";
        assert_eq!(paste(text, false), text.as_bytes());
        assert_eq!(
            paste(text, true),
            [PASTE_START, text.as_bytes(), PASTE_END].concat()
        );
    }

    #[test]
    fn sgr_encoder_is_relative_one_based_and_preserves_reported_modifiers() {
        let mut report = event(MouseEventKind::Down(MouseButton::Left), 12, 7);
        report.modifiers = KeyModifiers::SHIFT | KeyModifiers::CONTROL;
        assert_eq!(
            mouse(
                report,
                Rect::new(10, 5, 20, 10),
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Sgr,
            ),
            Some(b"\x1b[<20;3;3M".to_vec())
        );
    }

    #[test]
    fn host_motion_capture_expands_only_when_the_child_requests_it() {
        for mode in [
            MouseProtocolMode::None,
            MouseProtocolMode::Press,
            MouseProtocolMode::PressRelease,
        ] {
            assert_eq!(host_capture(mode), MouseCapture::PressRelease);
        }
        assert_eq!(
            host_capture(MouseProtocolMode::ButtonMotion),
            MouseCapture::ButtonMotion
        );
        assert_eq!(
            host_capture(MouseProtocolMode::AnyMotion),
            MouseCapture::AnyMotion
        );
    }

    #[test]
    fn legacy_mouse_encodes_press_release_wheel_and_boundaries() {
        let area = Rect::new(4, 3, 223, 223);
        assert_eq!(
            mouse(
                event(MouseEventKind::Down(MouseButton::Middle), 4, 3),
                area,
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Default,
            ),
            Some(vec![0x1b, b'[', b'M', 33, 33, 33])
        );
        assert_eq!(
            mouse(
                event(MouseEventKind::Up(MouseButton::Left), 5, 4),
                area,
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Default,
            ),
            Some(vec![0x1b, b'[', b'M', 35, 34, 34])
        );
        assert_eq!(
            mouse(
                event(MouseEventKind::ScrollDown, 226, 225),
                area,
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Default,
            ),
            Some(vec![0x1b, b'[', b'M', 97, 255, 255])
        );
        assert!(
            mouse(
                event(MouseEventKind::ScrollDown, 227, 225),
                Rect::new(4, 3, 224, 223),
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Default,
            )
            .is_none()
        );
    }

    #[test]
    fn mouse_requires_the_viewport_and_the_childs_requested_event_class() {
        let area = Rect::new(10, 5, 20, 10);
        let drag = event(MouseEventKind::Drag(MouseButton::Left), 10, 5);
        assert!(
            mouse(
                drag,
                area,
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Sgr
            )
            .is_none()
        );
        assert_eq!(
            mouse(
                drag,
                area,
                MouseProtocolMode::ButtonMotion,
                MouseProtocolEncoding::Sgr
            ),
            Some(b"\x1b[<32;1;1M".to_vec())
        );
        assert!(
            mouse(
                event(MouseEventKind::Down(MouseButton::Left), 9, 5),
                area,
                MouseProtocolMode::AnyMotion,
                MouseProtocolEncoding::Sgr,
            )
            .is_none()
        );
        assert!(
            mouse(
                event(MouseEventKind::Down(MouseButton::Left), 10, 5),
                area,
                MouseProtocolMode::None,
                MouseProtocolEncoding::Sgr,
            )
            .is_none()
        );
    }

    #[test]
    fn utf8_mouse_extends_coordinates_past_the_legacy_limit() {
        let bytes = mouse(
            event(MouseEventKind::Down(MouseButton::Left), 299, 0),
            Rect::new(0, 0, 300, 1),
            MouseProtocolMode::Press,
            MouseProtocolEncoding::Utf8,
        )
        .unwrap();
        assert_eq!(&bytes[..4], b"\x1b[M ");
        assert_eq!(std::str::from_utf8(&bytes[4..]).unwrap(), "Ō!");
        assert!(
            mouse(
                event(MouseEventKind::Down(MouseButton::Left), 2014, 0),
                Rect::new(0, 0, 2015, 1),
                MouseProtocolMode::Press,
                MouseProtocolEncoding::Utf8,
            )
            .is_some()
        );
        assert!(
            mouse(
                event(MouseEventKind::Down(MouseButton::Left), 2015, 0),
                Rect::new(0, 0, 2016, 1),
                MouseProtocolMode::Press,
                MouseProtocolEncoding::Utf8,
            )
            .is_none()
        );
    }
}
