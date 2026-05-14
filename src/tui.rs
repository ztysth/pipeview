use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use serde::Deserialize;

use crate::analysis::{Summary, summarize};
use crate::model::{Instruction, KeyValue, Stage, Trace};

mod keys;
pub use keys::parse_jump_target;

const DEFAULT_CELL_WIDTH: u16 = 5;
const MIN_CELL_WIDTH: u16 = 2;
const MAX_CELL_WIDTH: u16 = 18;
const DEFAULT_STAGE_COLORS: [Color; 12] = [
    Color::Rgb(0x0f, 0x76, 0x68),
    Color::Rgb(0x1d, 0x4e, 0x89),
    Color::Rgb(0x7c, 0x3a, 0x00),
    Color::Rgb(0x5b, 0x2a, 0x86),
    Color::Rgb(0x8a, 0x2d, 0x3b),
    Color::Rgb(0x2f, 0x5d, 0x50),
    Color::Rgb(0x54, 0x48, 0x2f),
    Color::Rgb(0x3f, 0x4a, 0x7a),
    Color::Rgb(0x6b, 0x3f, 0x5f),
    Color::Rgb(0x24, 0x5c, 0x68),
    Color::Rgb(0x67, 0x4a, 0x2f),
    Color::Rgb(0x3e, 0x5c, 0x35),
];

#[derive(Debug)]
pub(super) struct App {
    file_name: String,
    summary: Summary,
    rows: Vec<TimelineRow>,
    pub(super) selected_row: usize,
    pub(super) cycle_offset: u64,
    pub(super) cell_width: u16,
    pub(super) overlay: Overlay,
    pub(super) jump_input: String,
    pub(super) status: String,
    theme: Theme,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineRow {
    pub inst_id: u64,
    pub label: String,
    pub cells: BTreeMap<u64, TimelineCell>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineCell {
    pub stage: String,
    pub lane: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineRun {
    pub offset: u64,
    pub width: u64,
    pub cell: Option<TimelineCell>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Overlay {
    None,
    Info,
    Help,
    Jump,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    color_mode: ColorMode,
    stage_styles: BTreeMap<String, StageStyle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Default,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct StageStyle {
    fg: Color,
    bg: Option<Color>,
    alpha: f32,
}

#[derive(Debug, Deserialize)]
struct ThemeConfig {
    stages: BTreeMap<String, StageThemeConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StageThemeConfig {
    Fill(String),
    Style {
        fg: Option<String>,
        bg: Option<String>,
        alpha: Option<f32>,
    },
}

impl Theme {
    pub fn new(color_mode: ColorMode) -> Self {
        Self {
            color_mode,
            stage_styles: BTreeMap::new(),
        }
    }

    pub fn from_json_str(input: &str) -> Result<Self> {
        let config: ThemeConfig = serde_json::from_str(input)?;
        let stage_styles = config
            .stages
            .into_iter()
            .map(|(stage, config)| {
                let style = parse_stage_style(config)
                    .with_context(|| format!("invalid style for stage `{stage}`"))?;
                Ok((stage, style))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;

        Ok(Self {
            color_mode: ColorMode::Default,
            stage_styles,
        })
    }

    pub fn with_default_stage_colors(mut self, stages: &[Stage]) -> Self {
        if self.color_mode == ColorMode::None {
            return self;
        }

        for (index, stage) in stages.iter().enumerate() {
            self.stage_styles
                .entry(stage.id.clone())
                .or_insert(StageStyle {
                    fg: Color::White,
                    bg: Some(DEFAULT_STAGE_COLORS[index % DEFAULT_STAGE_COLORS.len()]),
                    alpha: 1.0,
                });
        }

        self
    }

    pub fn style_for_stage(&self, stage: &str) -> Style {
        if self.color_mode == ColorMode::None {
            return Style::default();
        }

        self.stage_styles
            .get(stage)
            .copied()
            .map_or_else(Style::default, StageStyle::into_style)
    }
}

impl StageStyle {
    fn into_style(self) -> Style {
        let style = Style::default().fg(self.fg);
        if self.alpha <= 0.0 {
            style
        } else {
            self.bg.map_or(style, |bg| style.bg(bg))
        }
    }
}

impl App {
    fn new(path: &Path, trace: Trace, theme: Theme) -> Self {
        let summary = summarize(&trace);
        let cycle_offset = summary.cycle_start.unwrap_or(0);
        let theme = theme.with_default_stage_colors(&trace.stages);
        Self {
            file_name: path.display().to_string(),
            summary,
            rows: build_timeline_rows(&trace),
            selected_row: 0,
            cycle_offset,
            cell_width: DEFAULT_CELL_WIDTH,
            overlay: Overlay::None,
            jump_input: String::new(),
            status: "?: help  i: info  g: jump  Esc: close panel/quit  Ctrl +/-: zoom  q: quit"
                .to_owned(),
            theme,
        }
    }

    pub(super) fn selected_row(&self) -> Option<&TimelineRow> {
        self.rows.get(self.selected_row)
    }

    pub(super) fn move_up(&mut self) {
        self.selected_row = self.selected_row.saturating_sub(1);
    }

    pub(super) fn move_down(&mut self) {
        if self.selected_row + 1 < self.rows.len() {
            self.selected_row += 1;
        }
    }

    pub(super) fn move_left(&mut self) {
        self.cycle_offset = self.cycle_offset.saturating_sub(1);
    }

    pub(super) fn move_right(&mut self) {
        self.cycle_offset = self.cycle_offset.saturating_add(1);
    }

    pub(super) fn jump_to_row_last_cycle(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        if let Some(last_cycle) = row.cells.keys().next_back().copied() {
            self.cycle_offset = last_cycle;
            self.status = format!(
                "jumped to last cycle {last_cycle} for row {}",
                self.selected_row + 1
            );
        }
    }

    pub(super) fn zoom_in(&mut self) {
        if self.cell_width < MAX_CELL_WIDTH {
            self.cell_width += 1;
        }
        self.status = format!("zoom: {} columns/cycle", self.cell_width);
    }

    pub(super) fn zoom_out(&mut self) {
        if self.cell_width > MIN_CELL_WIDTH {
            self.cell_width -= 1;
        }
        self.status = format!("zoom: {} columns/cycle", self.cell_width);
    }

    pub(super) fn begin_jump(&mut self) {
        self.overlay = Overlay::Jump;
        self.jump_input.clear();
        self.status = "jump: enter row,cycle then Enter; Esc cancels".to_owned();
    }

    pub(super) fn push_jump_char(&mut self, ch: char) {
        if ch.is_ascii_digit() || matches!(ch, ',' | ':' | ' ') {
            self.jump_input.push(ch);
        }
    }

    pub(super) fn pop_jump_char(&mut self) {
        self.jump_input.pop();
    }

    pub(super) fn apply_jump(&mut self) {
        match parse_jump_target(&self.jump_input) {
            Some((row, cycle)) => {
                self.selected_row = row.saturating_sub(1).min(self.rows.len().saturating_sub(1));
                self.cycle_offset = cycle;
                self.overlay = Overlay::None;
                self.status = format!("jumped to row {} cycle {}", self.selected_row + 1, cycle);
            }
            None => {
                self.status = "invalid jump target; use row,cycle".to_owned();
            }
        }
    }
}

pub fn run(path: &Path, trace: Trace, theme: Theme) -> Result<()> {
    let mut terminal = TerminalSession::start()?;
    let mut app = App::new(path, trace, theme);
    let result = run_loop(terminal.terminal(), &mut app);
    terminal.restore()?;
    result
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| render(frame, app))?;

        if event::poll(Duration::from_millis(100))? && !keys::handle_event(app, event::read()?) {
            break;
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
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, vertical[0], app);
    render_timeline(frame, vertical[1], app);
    render_status(frame, vertical[2], app);
    render_overlay(frame, area, app);
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
    let visible_cycles = visible_cycle_count(area.width, app.cell_width);
    let row_limit = area.height.saturating_sub(3) as usize;
    let start = app.selected_row.saturating_sub(row_limit.saturating_sub(1));
    let mut lines = Vec::with_capacity(row_limit + 1);
    lines.push(timeline_header(
        app.cycle_offset,
        visible_cycles,
        app.cell_width,
    ));
    lines.extend(
        app.rows
            .iter()
            .enumerate()
            .skip(start)
            .take(row_limit)
            .map(|(index, row)| {
                timeline_row(
                    row,
                    app.cycle_offset,
                    visible_cycles,
                    app.cell_width,
                    index == app.selected_row,
                    &app.theme,
                )
            }),
    );

    let timeline =
        Paragraph::new(lines).block(Block::default().title("Timeline").borders(Borders::ALL));
    frame.render_widget(timeline, area);
}

fn timeline_header(cycle_offset: u64, visible_cycles: u64, cell_width: u16) -> Line<'static> {
    let mut spans = Vec::with_capacity(visible_cycles as usize + 1);
    spans.push(Span::styled(
        fit_left("inst", 18),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    ));
    spans.extend((0..visible_cycles).map(|offset| {
        Span::styled(
            fit_center(&(cycle_offset + offset).to_string(), cell_width as usize),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    }));
    Line::from(spans)
}

fn timeline_row(
    row: &TimelineRow,
    cycle_offset: u64,
    visible_cycles: u64,
    cell_width: u16,
    selected: bool,
    theme: &Theme,
) -> Line<'static> {
    let label_style = if selected {
        Style::default().fg(Color::Black).bg(Color::White)
    } else {
        Style::default()
    };
    let mut spans = vec![Span::styled(fit_left(&row.label, 18), label_style)];
    spans.extend(timeline_run_spans(
        row,
        cycle_offset,
        visible_cycles,
        cell_width,
        theme,
    ));
    Line::from(spans)
}

fn timeline_run_spans(
    row: &TimelineRow,
    cycle_offset: u64,
    visible_cycles: u64,
    cell_width: u16,
    theme: &Theme,
) -> Vec<Span<'static>> {
    timeline_runs(row, cycle_offset, visible_cycles)
        .into_iter()
        .map(|run| {
            let span_width = (run.width * u64::from(cell_width)) as usize;
            match run.cell {
                Some(cell) => Span::styled(
                    fit_center(&cell.label, span_width),
                    theme.style_for_stage(&cell.stage),
                ),
                None => Span::raw(" ".repeat(span_width)),
            }
        })
        .collect()
}

pub fn timeline_runs(
    row: &TimelineRow,
    cycle_offset: u64,
    visible_cycles: u64,
) -> Vec<TimelineRun> {
    let mut runs = Vec::new();
    let mut offset = 0;

    while offset < visible_cycles {
        let start_cycle = cycle_offset + offset;
        let cell = row.cells.get(&start_cycle);
        let mut width = 1;

        while offset + width < visible_cycles
            && same_timeline_cell(cell, row.cells.get(&(start_cycle + width)))
        {
            width += 1;
        }

        runs.push(TimelineRun {
            offset,
            width,
            cell: cell.cloned(),
        });
        offset += width;
    }

    runs
}

fn same_timeline_cell(left: Option<&TimelineCell>, right: Option<&TimelineCell>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        (None, None) => true,
        _ => false,
    }
}

fn fit_left(value: &str, width: usize) -> String {
    let truncated = truncate_chars(value, width);
    format!("{truncated:<width$}")
}

fn fit_center(value: &str, width: usize) -> String {
    let truncated = truncate_chars(value, width);
    let padding = width.saturating_sub(truncated.chars().count());
    let left = padding / 2;
    let right = padding - left;
    format!("{}{}{}", " ".repeat(left), truncated, " ".repeat(right))
}

fn truncate_chars(value: &str, width: usize) -> String {
    value.chars().take(width).collect()
}

fn render_status(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let text = match app.overlay {
        Overlay::Jump => format!("jump row,cycle: {}", app.jump_input),
        _ => app.status.clone(),
    };
    frame.render_widget(Paragraph::new(text), area);
}

fn render_overlay(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    match app.overlay {
        Overlay::None => {}
        Overlay::Info => render_info_overlay(frame, centered_rect(area, 76, 14), app),
        Overlay::Help => render_help_overlay(frame, centered_rect(area, 76, 12)),
        Overlay::Jump => render_jump_overlay(frame, centered_rect(area, 58, 5), app),
    }
}

fn render_info_overlay(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    frame.render_widget(Clear, area);
    let selected = app
        .selected_row()
        .map(|row| format!("selected: {}", row.label))
        .unwrap_or_else(|| "selected: none".to_owned());
    let lines = vec![
        Line::from(selected),
        Line::from(format!(
            "row: {} / {}    cycle offset: {}    zoom: {}",
            app.selected_row + 1,
            app.rows.len(),
            app.cycle_offset,
            app.cell_width
        )),
        Line::from(format!(
            "instructions: {}  retired: {}  spans: {}  IPC: {}",
            app.summary.instruction_count,
            app.summary.retired_count,
            app.summary.span_count,
            app.summary
                .ipc
                .map_or_else(|| "n/a".to_owned(), |ipc| format!("{ipc:.3}"))
        )),
        Line::from(format_count_entries("top", &app.summary.top_bottlenecks)),
        Line::from(format_map_counts("stalls", &app.summary.stall_reasons)),
        Line::from(format_map_counts("flush", &app.summary.flush_reasons)),
        Line::from(format_map_counts("replay", &app.summary.replay_reasons)),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(Color::Black).fg(Color::White))
            .block(Block::default().title("Info").borders(Borders::ALL)),
        area,
    );
}

fn render_help_overlay(frame: &mut ratatui::Frame<'_>, area: Rect) {
    frame.render_widget(Clear, area);
    let lines = vec![
        Line::from("Esc close panel    q quit    arrows navigate"),
        Line::from("End jump to selected row last block"),
        Line::from("g jump to row,cycle    i toggle info    ? toggle help"),
        Line::from("Ctrl + / Ctrl = zoom in    Ctrl - zoom out"),
        Line::from("Ctrl + mouse wheel zooms when the terminal reports wheel modifiers"),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(Color::Black).fg(Color::White))
            .block(Block::default().title("Keys").borders(Borders::ALL)),
        area,
    );
}

fn render_jump_overlay(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    frame.render_widget(Clear, area);
    let lines = vec![
        Line::from("Enter row,cycle, e.g. 120,450. Esc closes this panel."),
        Line::from(format!("> {}", app.jump_input)),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(Color::Black).fg(Color::White))
            .block(Block::default().title("Jump").borders(Borders::ALL)),
        area,
    );
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

pub fn visible_cycle_count(width: u16, cell_width: u16) -> u64 {
    let usable = width.saturating_sub(20);
    (usable / cell_width.max(1)).max(1) as u64
}

fn format_count_entries(label: &str, counts: &[crate::analysis::CountEntry]) -> String {
    if counts.is_empty() {
        return format!("{label}: none");
    }

    let values = counts
        .iter()
        .take(4)
        .map(|entry| format!("{}={}", entry.key, entry.count))
        .collect::<Vec<_>>()
        .join(" ");
    format!("{label}: {values}")
}

fn format_map_counts(label: &str, counts: &BTreeMap<String, u64>) -> String {
    if counts.is_empty() {
        return format!("{label}: none");
    }

    let values = counts
        .iter()
        .take(4)
        .map(|(key, count)| format!("{key}={count}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!("{label}: {values}")
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

fn timeline_cell(stage: &str, lane: &str) -> TimelineCell {
    let label = if lane == "main" {
        stage.to_owned()
    } else {
        format!("{stage}/{lane}")
    };

    TimelineCell {
        stage: stage.to_owned(),
        lane: lane.to_owned(),
        label,
    }
}

fn parse_stage_style(config: StageThemeConfig) -> Result<StageStyle> {
    match config {
        StageThemeConfig::Fill(color) => Ok(StageStyle {
            fg: Color::White,
            bg: Some(parse_color(&color)?),
            alpha: 1.0,
        }),
        StageThemeConfig::Style { fg, bg, alpha } => {
            let alpha = alpha.unwrap_or(1.0);
            if !(0.0..=1.0).contains(&alpha) {
                bail!("alpha must be in the range 0.0..=1.0");
            }

            let fg = fg
                .as_deref()
                .map(parse_color)
                .transpose()?
                .unwrap_or(Color::White);
            let bg = bg.as_deref().map(parse_color).transpose()?;

            Ok(StageStyle { fg, bg, alpha })
        }
    }
}

fn parse_color(input: &str) -> Result<Color> {
    let normalized = input.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "black" => Ok(Color::Black),
        "red" => Ok(Color::Red),
        "green" => Ok(Color::Green),
        "yellow" => Ok(Color::Yellow),
        "blue" => Ok(Color::Blue),
        "magenta" => Ok(Color::Magenta),
        "cyan" => Ok(Color::Cyan),
        "gray" | "grey" => Ok(Color::Gray),
        "darkgray" | "darkgrey" | "dark_gray" | "dark_grey" => Ok(Color::DarkGray),
        "lightred" | "light_red" => Ok(Color::LightRed),
        "lightgreen" | "light_green" => Ok(Color::LightGreen),
        "lightyellow" | "light_yellow" => Ok(Color::LightYellow),
        "lightblue" | "light_blue" => Ok(Color::LightBlue),
        "lightmagenta" | "light_magenta" => Ok(Color::LightMagenta),
        "lightcyan" | "light_cyan" => Ok(Color::LightCyan),
        "white" => Ok(Color::White),
        _ => parse_hex_color(&normalized),
    }
}

fn parse_hex_color(input: &str) -> Result<Color> {
    let Some(hex) = input.strip_prefix('#') else {
        bail!("expected a named color or #rrggbb")
    };
    if hex.len() != 6 {
        bail!("hex colors must use #rrggbb")
    }

    let red = u8::from_str_radix(&hex[0..2], 16)?;
    let green = u8::from_str_radix(&hex[2..4], 16)?;
    let blue = u8::from_str_radix(&hex[4..6], 16)?;
    Ok(Color::Rgb(red, green, blue))
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    restored: bool,
}

impl TerminalSession {
    fn start() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
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
            execute!(
                self.terminal.backend_mut(),
                DisableMouseCapture,
                LeaveAlternateScreen
            )?;
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
