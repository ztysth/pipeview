use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
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
use ratatui::widgets::{Block, Borders, Paragraph};
use serde::Deserialize;

use crate::analysis::{Summary, summarize};
use crate::model::{Instruction, KeyValue, Stage, Trace};

const DEFAULT_CYCLE_WIDTH: u64 = 24;
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
struct App {
    file_name: String,
    summary: Summary,
    rows: Vec<TimelineRow>,
    selected_row: usize,
    cycle_offset: u64,
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
            theme,
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
    let row_limit = area.height.saturating_sub(3) as usize;
    let start = app.selected_row.saturating_sub(row_limit.saturating_sub(1));
    let mut lines = Vec::with_capacity(row_limit + 1);
    lines.push(timeline_header(app.cycle_offset, visible_cycles));
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
                    index == app.selected_row,
                    &app.theme,
                )
            }),
    );

    let timeline =
        Paragraph::new(lines).block(Block::default().title("Timeline").borders(Borders::ALL));
    frame.render_widget(timeline, area);
}

fn timeline_header(cycle_offset: u64, visible_cycles: u64) -> Line<'static> {
    let mut spans = Vec::with_capacity(visible_cycles as usize + 1);
    spans.push(Span::styled(
        fit_left("inst", 18),
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    ));
    spans.extend((0..visible_cycles).map(|offset| {
        Span::styled(
            fit_center(&(cycle_offset + offset).to_string(), 5),
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
    selected: bool,
    theme: &Theme,
) -> Line<'static> {
    let label_style = if selected {
        Style::default().fg(Color::Black).bg(Color::White)
    } else {
        Style::default()
    };
    let mut spans = vec![Span::styled(fit_left(&row.label, 18), label_style)];
    spans.extend(timeline_run_spans(row, cycle_offset, visible_cycles, theme));
    Line::from(spans)
}

fn timeline_run_spans(
    row: &TimelineRow,
    cycle_offset: u64,
    visible_cycles: u64,
    theme: &Theme,
) -> Vec<Span<'static>> {
    timeline_runs(row, cycle_offset, visible_cycles)
        .into_iter()
        .map(|run| {
            let span_width = (run.width * 5) as usize;
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
