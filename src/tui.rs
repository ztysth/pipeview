use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};

use crate::analysis::{Summary, summarize};
use crate::model::{Instruction, KeyValue, Trace};

const DEFAULT_CYCLE_WIDTH: u64 = 24;

#[derive(Debug)]
struct App {
    file_name: String,
    summary: Summary,
    rows: Vec<TimelineRow>,
    selected_row: usize,
    cycle_offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineRow {
    pub inst_id: u64,
    pub label: String,
    pub cells: BTreeMap<u64, String>,
}

impl App {
    fn new(path: &Path, trace: Trace) -> Self {
        let summary = summarize(&trace);
        let cycle_offset = summary.cycle_start.unwrap_or(0);
        Self {
            file_name: path.display().to_string(),
            summary,
            rows: build_timeline_rows(&trace),
            selected_row: 0,
            cycle_offset,
        }
    }

    fn selected_row(&self) -> Option<&TimelineRow> {
        self.rows.get(self.selected_row)
    }

    fn move_up(&mut self) {
        self.selected_row = self.selected_row.saturating_sub(1);
    }

    fn move_down(&mut self) {
        if self.selected_row + 1 < self.rows.len() {
            self.selected_row += 1;
        }
    }

    fn move_left(&mut self) {
        self.cycle_offset = self.cycle_offset.saturating_sub(1);
    }

    fn move_right(&mut self) {
        self.cycle_offset = self.cycle_offset.saturating_add(1);
    }
}

pub fn run(path: &Path, trace: Trace) -> Result<()> {
    let mut terminal = TerminalSession::start()?;
    let mut app = App::new(path, trace);
    let result = run_loop(terminal.terminal(), &mut app);
    terminal.restore()?;
    result
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| render(frame, app))?;

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Up => app.move_up(),
                KeyCode::Down => app.move_down(),
                KeyCode::Left => app.move_left(),
                KeyCode::Right => app.move_right(),
                _ => {}
            }
        }
    }

    Ok(())
}

fn render(frame: &mut ratatui::Frame<'_>, app: &App) {
    let area = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(5),
        ])
        .split(area);

    render_header(frame, vertical[0], app);
    render_timeline(frame, vertical[1], app);
    render_details(frame, vertical[2], app);
}

fn render_header(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let ipc = app
        .summary
        .ipc
        .map_or_else(|| "n/a".to_owned(), |value| format!("{value:.3}"));
    let cycle_range = match (app.summary.cycle_start, app.summary.cycle_end) {
        (Some(start), Some(end)) => format!("{start}-{end}"),
        _ => "n/a".to_owned(),
    };
    let title = format!(
        "{} | inst {} retired {} IPC {} cycles {}",
        app.file_name, app.summary.instruction_count, app.summary.retired_count, ipc, cycle_range
    );
    let header = Paragraph::new(title).block(Block::default().borders(Borders::ALL));
    frame.render_widget(header, area);
}

fn render_timeline(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let visible_cycles = visible_cycle_count(area.width);
    let mut widths = Vec::with_capacity(visible_cycles as usize + 1);
    widths.push(Constraint::Length(18));
    widths.extend((0..visible_cycles).map(|_| Constraint::Length(5)));

    let header_cells = std::iter::once(Cell::from("inst")).chain(
        (0..visible_cycles).map(|offset| Cell::from((app.cycle_offset + offset).to_string())),
    );
    let header = Row::new(header_cells).style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let row_limit = area.height.saturating_sub(3) as usize;
    let start = app.selected_row.saturating_sub(row_limit.saturating_sub(1));
    let rows = app
        .rows
        .iter()
        .enumerate()
        .skip(start)
        .take(row_limit)
        .map(|(index, row)| {
            let style = if index == app.selected_row {
                Style::default().fg(Color::Black).bg(Color::White)
            } else {
                Style::default()
            };
            let cells = std::iter::once(Cell::from(row.label.clone())).chain(
                (0..visible_cycles).map(|offset| {
                    Cell::from(
                        row.cells
                            .get(&(app.cycle_offset + offset))
                            .cloned()
                            .unwrap_or_default(),
                    )
                }),
            );
            Row::new(cells).style(style)
        });

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().title("Timeline").borders(Borders::ALL));
    frame.render_widget(table, area);
}

fn render_details(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let selected = app
        .selected_row()
        .map(|row| format!("selected: {}", row.label))
        .unwrap_or_else(|| "selected: none".to_owned());
    let stall = if app.summary.stall_reasons.is_empty() {
        "stalls: none".to_owned()
    } else {
        let reasons = app
            .summary
            .stall_reasons
            .iter()
            .map(|(reason, count)| format!("{reason}={count}"))
            .collect::<Vec<_>>()
            .join(" ");
        format!("stalls: {reasons}")
    };
    let lines = vec![
        Line::from(vec![
            Span::styled("q", Style::default().fg(Color::Yellow)),
            Span::raw("/Esc quit  "),
            Span::styled("arrows", Style::default().fg(Color::Yellow)),
            Span::raw(" navigate"),
        ]),
        Line::from(selected),
        Line::from(stall),
    ];
    let details =
        Paragraph::new(lines).block(Block::default().title("Details").borders(Borders::ALL));
    frame.render_widget(details, area);
}

fn visible_cycle_count(width: u16) -> u64 {
    let usable = width.saturating_sub(20);
    let count = (usable / 5).max(1) as u64;
    count.min(DEFAULT_CYCLE_WIDTH)
}

pub fn build_timeline_rows(trace: &Trace) -> Vec<TimelineRow> {
    let instruction_labels = trace
        .instructions
        .iter()
        .map(|instruction| (instruction.inst_id, instruction_label(instruction)))
        .collect::<BTreeMap<_, _>>();

    let mut rows = trace
        .instructions
        .iter()
        .map(|instruction| TimelineRow {
            inst_id: instruction.inst_id,
            label: instruction_labels
                .get(&instruction.inst_id)
                .cloned()
                .unwrap_or_else(|| format!("#{}", instruction.inst_id)),
            cells: BTreeMap::new(),
        })
        .collect::<Vec<_>>();
    let mut row_indexes = rows
        .iter()
        .enumerate()
        .map(|(index, row)| (row.inst_id, index))
        .collect::<BTreeMap<_, _>>();

    for span in &trace.spans {
        let Some(row_index) = row_indexes.get(&span.inst_id).copied() else {
            continue;
        };
        for cycle in span.cycle..span.cycle + span.duration {
            rows[row_index]
                .cells
                .insert(cycle, timeline_cell(&span.stage, &span.lane));
        }
    }

    rows.sort_by_key(|row| row.inst_id);
    row_indexes.clear();
    rows
}

fn instruction_label(instruction: &Instruction) -> String {
    let pc = attr_value(&instruction.attrs, "pc");
    let asm = attr_value(&instruction.attrs, "asm");
    match (pc, asm) {
        (Some(pc), Some(asm)) => format!("#{} {pc} {asm}", instruction.inst_id),
        (Some(pc), None) => format!("#{} {pc}", instruction.inst_id),
        (None, Some(asm)) => format!("#{} {asm}", instruction.inst_id),
        (None, None) => format!("#{}", instruction.inst_id),
    }
}

fn attr_value<'a>(attrs: &'a [KeyValue], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|attr| attr.key == key)
        .map(|attr| attr.value.as_str())
}

fn timeline_cell(stage: &str, lane: &str) -> String {
    if lane == "main" {
        stage.to_owned()
    } else {
        format!("{stage}/{lane}")
    }
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    restored: bool,
}

impl TerminalSession {
    fn start() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            restored: false,
        })
    }

    fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<io::Stdout>> {
        &mut self.terminal
    }

    fn restore(&mut self) -> Result<()> {
        if !self.restored {
            disable_raw_mode()?;
            execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
            self.terminal.show_cursor()?;
            self.restored = true;
        }
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::parse_plog;

    use super::{build_timeline_rows, visible_cycle_count};

    #[test]
    fn builds_timeline_rows_from_spans() {
        let trace = parse_plog(concat!(
            "PLOG\t1\n",
            "STAGE\tIF\tFetch\n",
            "STAGE\tID\tDecode\n",
            "LANE\tmain\tMain\n",
            "LANE\tstall\tStall\n",
            "I\t1\tpc=0x80000000\tasm=lw\n",
            "B\t1\t1\t1\tmain\tIF\n",
            "B\t2\t2\t1\tstall\tID\treason=load_use\n",
            "R\t4\t1\tretire\n",
        ))
        .expect("valid trace");

        let rows = build_timeline_rows(&trace);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "#1 0x80000000 lw");
        assert_eq!(rows[0].cells[&1], "IF");
        assert_eq!(rows[0].cells[&2], "ID/stall");
        assert_eq!(rows[0].cells[&3], "ID/stall");
    }

    #[test]
    fn keeps_at_least_one_visible_cycle() {
        assert_eq!(visible_cycle_count(0), 1);
        assert_eq!(visible_cycle_count(24), 1);
    }
}
