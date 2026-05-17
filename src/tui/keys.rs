use crossterm::event::{Event, KeyCode, KeyModifiers, MouseEventKind};

use super::{App, Overlay};

pub(super) fn handle_event(app: &mut App, event: Event) -> bool {
    match event {
        Event::Key(key) if app.overlay == Overlay::Jump => {
            match key.code {
                KeyCode::Esc => {
                    app.overlay = Overlay::None;
                    app.status = "panel closed".to_owned();
                }
                KeyCode::Enter => app.apply_jump(),
                KeyCode::Backspace => app.pop_jump_char(),
                KeyCode::Char(ch) => app.push_jump_char(ch),
                _ => {}
            }
            true
        }
        Event::Key(key) => match key.code {
            KeyCode::Char('q') => false,
            KeyCode::Esc if app.overlay != Overlay::None => {
                app.overlay = Overlay::None;
                app.status = "panel closed".to_owned();
                true
            }
            KeyCode::Esc => false,
            KeyCode::Up => {
                app.move_up();
                true
            }
            KeyCode::Down => {
                app.move_down();
                true
            }
            KeyCode::Left => {
                app.move_left();
                true
            }
            KeyCode::Right => {
                app.move_right();
                true
            }
            KeyCode::End => {
                app.jump_to_row_last_cycle();
                true
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                app.zoom_in();
                true
            }
            KeyCode::Char('-') => {
                app.zoom_out();
                true
            }
            KeyCode::Char('?') => {
                app.overlay = if app.overlay == Overlay::Help {
                    Overlay::None
                } else {
                    Overlay::Help
                };
                true
            }
            KeyCode::Char('i') => {
                app.overlay = if app.overlay == Overlay::Info {
                    Overlay::None
                } else {
                    Overlay::Info
                };
                true
            }
            KeyCode::Char('d') => {
                app.overlay = if app.overlay == Overlay::Detail {
                    Overlay::None
                } else {
                    Overlay::Detail
                };
                true
            }
            KeyCode::Char('g') => {
                app.begin_jump();
                true
            }
            _ => true,
        },
        Event::Mouse(mouse) if mouse.modifiers.contains(KeyModifiers::CONTROL) => {
            match mouse.kind {
                MouseEventKind::ScrollUp => app.zoom_in(),
                MouseEventKind::ScrollDown => app.zoom_out(),
                _ => {}
            }
            true
        }
        Event::Mouse(mouse) => {
            match mouse.kind {
                MouseEventKind::ScrollUp => app.move_left(),
                MouseEventKind::ScrollDown => app.move_right(),
                _ => {}
            }
            true
        }
        _ => true,
    }
}

pub fn parse_jump_target(input: &str) -> Option<(usize, u64)> {
    let mut parts = input.split([',', ':', ' ']).filter(|part| !part.is_empty());
    let row = parts.next()?.parse::<usize>().ok()?;
    let cycle = parts.next()?.parse::<u64>().ok()?;
    if row == 0 || parts.next().is_some() {
        return None;
    }
    Some((row, cycle))
}
