use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};

use super::{App, Overlay};

type AppAction = fn(&mut App);

pub(super) enum Action {
    Quit,
    Redraw,
    Ignore,
}

pub(super) fn handle_event(app: &mut App, event: Event) -> Action {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Release => Action::Ignore,
        Event::Key(key) if app.overlay == Overlay::Jump => {
            match key.code {
                KeyCode::Esc => {
                    app.overlay = Overlay::None;
                    app.status.clear();
                }
                KeyCode::Enter => app.apply_jump(),
                KeyCode::Backspace => app.pop_jump_char(),
                KeyCode::Char(ch) => app.push_jump_char(ch),
                _ => return Action::Ignore,
            }
            Action::Redraw
        }
        Event::Key(key) => {
            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
            match key.code {
                KeyCode::Char('q') => return Action::Quit,
                KeyCode::Esc if app.overlay != Overlay::None => app.overlay = Overlay::None,
                KeyCode::Esc => return Action::Quit,
                KeyCode::Up | KeyCode::Char('k') => app.move_up(),
                KeyCode::Down | KeyCode::Char('j') => app.move_down(),
                KeyCode::PageUp => app.page_up(),
                KeyCode::PageDown => app.page_down(),
                KeyCode::Left if shift => app.page_left(),
                KeyCode::Right if shift => app.page_right(),
                KeyCode::Left | KeyCode::Char('h') => app.move_left(),
                KeyCode::Right | KeyCode::Char('l') => app.move_right(),
                KeyCode::Char('H') => app.page_left(),
                KeyCode::Char('L') => app.page_right(),
                KeyCode::Home => app.jump_to_row_first_cycle(),
                KeyCode::End => app.jump_to_row_last_cycle(),
                KeyCode::Char('+') | KeyCode::Char('=') => app.zoom_in(),
                KeyCode::Char('-') => app.zoom_out(),
                KeyCode::Char('?') => app.toggle_overlay(Overlay::Help),
                KeyCode::Char('i') => app.toggle_overlay(Overlay::Info),
                KeyCode::Char('d') => app.toggle_detail_overlay(),
                KeyCode::Char('g') => app.begin_jump(),
                _ => return Action::Ignore,
            }
            Action::Redraw
        }
        Event::Mouse(mouse) => {
            let (up, down): (AppAction, AppAction) =
                if mouse.modifiers.contains(KeyModifiers::CONTROL) {
                    (App::zoom_in, App::zoom_out)
                } else if mouse.modifiers.contains(KeyModifiers::ALT) {
                    (App::move_left, App::move_right)
                } else {
                    (App::move_up, App::move_down)
                };
            match mouse.kind {
                MouseEventKind::ScrollUp => up(app),
                MouseEventKind::ScrollDown => down(app),
                _ => return Action::Ignore,
            }
            Action::Redraw
        }
        Event::Resize(..) => Action::Redraw,
        _ => Action::Ignore,
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
