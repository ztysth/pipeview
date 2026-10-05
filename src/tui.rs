use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::io;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Color, Style};
use serde::Deserialize;

use crate::analysis::{Summary, SummaryOptions, summarize_for_tui};
use crate::model::{
    Instruction, InstructionOrder, KeyValue, RetireEvent, Span as ModelSpan, Stage, Trace,
};
use crate::plog_io::{
    InputFormat, read_konata_preview_trace, read_plog_preview_trace, read_trace,
    resolve_input_format,
};

mod keys;
mod render;
use keys::Action;
pub use keys::parse_jump_target;

const DEFAULT_CELL_WIDTH: u16 = 5;
const MIN_CELL_WIDTH: u16 = 1;
const KONATA_PREVIEW_INSTRUCTIONS: usize = 4_096;
const PLOG_PREVIEW_SPANS: usize = 64_000;
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
    view: TraceView,
    pub(super) selected_row: usize,
    pub(super) cycle_offset: u64,
    pub(super) cell_width: u16,
    pub(super) overlay: Overlay,
    pub(super) jump_input: String,
    pub(super) status: String,
    theme: Theme,
    detail_cache: BTreeMap<u64, InstructionDetail>,
    trace: Trace,
    preview: bool,
    label_width: usize,
    row_offset: Cell<usize>,
    viewport: Cell<Viewport>,
}

/// What the last frame could show; paging moves by this much.
#[derive(Debug, Clone, Copy)]
struct Viewport {
    rows: usize,
    cycles: u64,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            rows: 20,
            cycles: 20,
        }
    }
}

/// Per-instruction rows plus the record indexes the TUI needs for lookups.
///
/// Span and event indexes live in flat arrays sliced per row, so building the
/// view costs a few large allocations instead of several per instruction, and
/// queries stay proportional to the selected instruction.
#[derive(Debug)]
pub struct TraceView {
    order: InstructionOrder,
    rows: Vec<TimelineRowView>,
    span_order: Vec<u32>,
    event_offsets: Vec<u32>,
    event_order: Vec<u32>,
    retire_of_row: Vec<u32>,
}

const NO_RECORD: u32 = u32::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineRowView {
    inst_id: u64,
    instruction_index: usize,
    span_start: u32,
    span_end: u32,
    last_cycle: Option<u64>,
}

impl TimelineRowView {
    pub fn inst_id(&self) -> u64 {
        self.inst_id
    }

    pub fn span_count(&self) -> usize {
        (self.span_end - self.span_start) as usize
    }

    pub fn last_cycle(&self) -> Option<u64> {
        self.last_cycle
    }

    pub fn label(&self, trace: &Trace) -> String {
        instruction_label(&trace.instructions[self.instruction_index])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineRow {
    pub inst_id: u64,
    pub label: String,
    pub spans: Vec<TimelineSpan>,
    pub last_cycle: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineSpan {
    pub cycle: u64,
    pub duration: u64,
    pub cell: TimelineCell,
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
    Detail,
    Help,
    Jump,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionDetail {
    pub inst_id: u64,
    pub label: String,
    pub attrs: Vec<KeyValue>,
    pub spans: Vec<SpanDetail>,
    pub events: Vec<EventDetail>,
    pub retire: Option<RetireDetail>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanDetail {
    pub cycle: u64,
    pub duration: u64,
    pub stage: String,
    pub lane: String,
    pub attrs: Vec<KeyValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDetail {
    pub cycle: u64,
    pub event: String,
    pub attrs: Vec<KeyValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetireDetail {
    pub cycle: u64,
    pub status: String,
    pub attrs: Vec<KeyValue>,
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

    fn colored(&self) -> bool {
        self.color_mode != ColorMode::None
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
    fn new(path: &Path, trace: Trace, theme: Theme, summary_options: SummaryOptions) -> Self {
        let (summary, view, label_width) = thread::scope(|scope| {
            let summary = scope.spawn(|| summarize_for_tui(&trace, summary_options));
            let label_width = scope.spawn(|| max_label_width(&trace));
            let view = TraceView::new(&trace);
            (
                summary.join().expect("summary thread panicked"),
                view,
                label_width.join().expect("label width thread panicked"),
            )
        });
        let cycle_offset = summary.cycle_start.unwrap_or(0);
        let theme = theme.with_default_stage_colors(&trace.stages);
        Self {
            file_name: path.display().to_string(),
            summary,
            view,
            selected_row: 0,
            cycle_offset,
            cell_width: DEFAULT_CELL_WIDTH,
            overlay: Overlay::None,
            jump_input: String::new(),
            status: String::new(),
            theme,
            detail_cache: BTreeMap::new(),
            trace,
            preview: false,
            label_width,
            row_offset: Cell::new(0),
            viewport: Cell::new(Viewport::default()),
        }
    }

    pub(super) fn selected_row(&self) -> Option<&TimelineRowView> {
        self.view.rows.get(self.selected_row)
    }

    fn row_count(&self) -> usize {
        self.view.rows.len()
    }

    pub(super) fn selected_detail(&self) -> Option<&InstructionDetail> {
        self.selected_row()
            .and_then(|row| self.detail_cache.get(&row.inst_id))
    }

    fn preserve_view_state_from(&mut self, previous: &Self) {
        self.selected_row = previous
            .selected_row
            .min(self.row_count().saturating_sub(1));
        self.row_offset.set(previous.row_offset.get());
        self.cycle_offset = previous.cycle_offset;
        self.cell_width = previous.cell_width;
        self.overlay = previous.overlay;
        self.jump_input.clone_from(&previous.jump_input);

        if self.overlay == Overlay::Detail {
            self.ensure_selected_detail();
        }
    }

    pub(super) fn ensure_selected_detail(&mut self) {
        let Some(inst_id) = self.selected_row().map(|row| row.inst_id) else {
            return;
        };
        if self.detail_cache.contains_key(&inst_id) {
            return;
        }
        if let Some(detail) = self.view.instruction_detail(&self.trace, inst_id) {
            self.detail_cache.insert(inst_id, detail);
        }
    }

    pub(super) fn toggle_overlay(&mut self, overlay: Overlay) {
        self.overlay = if self.overlay == overlay {
            Overlay::None
        } else {
            overlay
        };
    }

    pub(super) fn toggle_detail_overlay(&mut self) {
        if self.overlay == Overlay::Detail {
            self.overlay = Overlay::None;
        } else {
            self.ensure_selected_detail();
            self.overlay = Overlay::Detail;
        }
    }

    pub(super) fn move_up(&mut self) {
        self.selected_row = self.selected_row.saturating_sub(1);
        self.refresh_detail();
    }

    pub(super) fn move_down(&mut self) {
        if self.selected_row + 1 < self.row_count() {
            self.selected_row += 1;
        }
        self.refresh_detail();
    }

    pub(super) fn page_up(&mut self) {
        let page = self.viewport.get().rows.max(1);
        self.selected_row = self.selected_row.saturating_sub(page);
        self.refresh_detail();
    }

    pub(super) fn page_down(&mut self) {
        let page = self.viewport.get().rows.max(1);
        self.selected_row = (self.selected_row + page).min(self.row_count().saturating_sub(1));
        self.refresh_detail();
    }

    fn refresh_detail(&mut self) {
        if self.overlay == Overlay::Detail {
            self.ensure_selected_detail();
        }
    }

    pub(super) fn page_left(&mut self) {
        self.cycle_offset = self
            .cycle_offset
            .saturating_sub(self.viewport.get().cycles.max(1));
    }

    pub(super) fn page_right(&mut self) {
        self.cycle_offset = self
            .cycle_offset
            .saturating_add(self.viewport.get().cycles.max(1));
    }

    pub(super) fn jump_to_row_first_cycle(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        if let Some(first_cycle) = self
            .view
            .row_spans(row, &self.trace)
            .next()
            .map(|span| span.cycle)
        {
            self.cycle_offset = first_cycle;
            self.status = format!("first cycle {first_cycle} of row {}", self.selected_row + 1);
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
        if let Some(last_cycle) = row.last_cycle {
            self.cycle_offset = last_cycle;
            self.status = format!("last cycle {last_cycle} of row {}", self.selected_row + 1);
        }
    }

    pub(super) fn zoom_in(&mut self) {
        self.cell_width = self.cell_width.saturating_add(1);
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
        self.status.clear();
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
                self.selected_row = row
                    .saturating_sub(1)
                    .min(self.row_count().saturating_sub(1));
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
    let mut app = App::new(
        path,
        trace,
        theme,
        SummaryOptions {
            experimental_bottlenecks: true,
        },
    );
    let result = run_loop(terminal.terminal(), &mut app);
    terminal.restore()?;
    result
}

pub fn run_path(
    path: &Path,
    theme: Theme,
    max_input_bytes: u64,
    input_format: InputFormat,
    summary_options: SummaryOptions,
) -> Result<()> {
    let mut terminal = TerminalSession::start()?;
    let (sender, receiver) = mpsc::channel();
    let path_buf = path.to_path_buf();
    let started_at = Instant::now();
    let profile_exit = env::var_os("PIPEVIEW_PROFILE_EXIT").is_some();
    let mut profile_line = None;
    let preview_format = resolve_input_format(path, input_format);

    let loader_theme = theme.clone();
    let theme_colored = theme.colored();
    thread::spawn(move || {
        // Summaries and row/record indexes are built here so the UI thread only
        // has to swap in a ready-to-render app.
        let result = read_trace(&path_buf, max_input_bytes, input_format)
            .map(|trace| App::new(&path_buf, trace, loader_theme, summary_options));
        let _ = sender.send(result);
    });

    let mut preview_app = match preview_format {
        InputFormat::Konata => {
            read_konata_preview_trace(path, max_input_bytes, KONATA_PREVIEW_INSTRUCTIONS).ok()
        }
        InputFormat::Plog => {
            read_plog_preview_trace(path, max_input_bytes, PLOG_PREVIEW_SPANS).ok()
        }
        InputFormat::Auto => unreachable!("input format must be resolved before preview parsing"),
    }
    .map(|trace| {
        let mut app = App::new(path, trace, theme, summary_options);
        app.preview = true;
        app.status = "full trace still loading…".to_owned();
        app
    });

    let mut preview_dirty = true;
    let result = loop {
        match receiver.try_recv() {
            Ok(Ok(mut app)) => {
                if let Some(preview) = preview_app.as_ref() {
                    app.preserve_view_state_from(preview);
                    app.status = "full trace loaded".to_owned();
                }
                if profile_exit {
                    terminal
                        .terminal()
                        .draw(|frame| render::render(frame, &app))?;
                    profile_line = Some(format!(
                        "PIPEVIEW_PROFILE first_draw_ms={} rows={} instructions={} spans={}",
                        started_at.elapsed().as_millis(),
                        app.row_count(),
                        app.summary.instruction_count,
                        app.summary.span_count
                    ));
                    break Ok(());
                }
                break run_loop(terminal.terminal(), &mut app);
            }
            Ok(Err(error)) => break Err(error),
            Err(mpsc::TryRecvError::Disconnected) => {
                break Err(anyhow!("PLog loader stopped unexpectedly"));
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }

        if let Some(app) = preview_app.as_mut() {
            if preview_dirty {
                terminal
                    .terminal()
                    .draw(|frame| render::render(frame, app))?;
                preview_dirty = false;
            }
            if profile_exit {
                profile_line = Some(format!(
                    "PIPEVIEW_PROFILE first_draw_ms={} rows={} instructions={} spans={} mode=preview",
                    started_at.elapsed().as_millis(),
                    app.row_count(),
                    app.summary.instruction_count,
                    app.summary.span_count
                ));
                break Ok(());
            }
        } else {
            terminal.terminal().draw(|frame| {
                render::render_loading(frame, path, started_at.elapsed(), theme_colored)
            })?;
        }

        if event::poll(Duration::from_millis(100))? {
            let input = event::read()?;
            if let Some(app) = preview_app.as_mut() {
                match keys::handle_event(app, input) {
                    Action::Quit => break Ok(()),
                    Action::Redraw => preview_dirty = true,
                    Action::Ignore => {}
                }
            } else if matches!(
                input,
                Event::Key(key) if matches!(key.code, KeyCode::Esc | KeyCode::Char('q'))
            ) {
                break Ok(());
            }
        }
    };

    terminal.restore()?;
    if let Some(line) = profile_line {
        println!("{line}");
    }
    result
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| render::render(frame, app))?;

        // The trace is static, so block until something changes the view, and
        // fold bursts (wheel scrolls, key repeat) into a single frame.
        let mut redraw = false;
        loop {
            match keys::handle_event(app, event::read()?) {
                Action::Quit => return Ok(()),
                Action::Redraw => redraw = true,
                Action::Ignore => {}
            }
            if redraw && !event::poll(Duration::ZERO)? {
                break;
            }
        }
    }
}

pub fn timeline_runs(
    row: &TimelineRow,
    cycle_offset: u64,
    visible_cycles: u64,
) -> Vec<TimelineRun> {
    let mut runs = Vec::new();
    let window_end = cycle_offset.saturating_add(visible_cycles);
    let mut cursor = cycle_offset;

    for span in &row.spans {
        let span_end = span.cycle.saturating_add(span.duration);
        if span_end <= cursor {
            continue;
        }
        if span.cycle >= window_end {
            break;
        }

        if span.cycle > cursor {
            push_timeline_run(
                &mut runs,
                cycle_offset,
                cursor,
                span.cycle.min(window_end) - cursor,
                None,
            );
            cursor = span.cycle;
            if cursor >= window_end {
                break;
            }
        }

        let clipped_end = span_end.min(window_end);
        if clipped_end > cursor {
            push_timeline_run(
                &mut runs,
                cycle_offset,
                cursor,
                clipped_end - cursor,
                Some(span.cell.clone()),
            );
            cursor = clipped_end;
        }
        if cursor >= window_end {
            break;
        }
    }

    if cursor < window_end {
        push_timeline_run(&mut runs, cycle_offset, cursor, window_end - cursor, None);
    }

    runs
}

fn push_timeline_run(
    runs: &mut Vec<TimelineRun>,
    cycle_offset: u64,
    cycle: u64,
    width: u64,
    cell: Option<TimelineCell>,
) {
    if width == 0 {
        return;
    }

    if let Some(last) = runs.last_mut()
        && last.cell == cell
    {
        last.width += width;
        return;
    }

    runs.push(TimelineRun {
        offset: cycle.saturating_sub(cycle_offset),
        width,
        cell,
    });
}

pub fn visible_cycle_count(width: u16, cell_width: u16) -> u64 {
    let usable = width.saturating_sub(20);
    (usable / cell_width.max(1)).max(1) as u64
}

pub fn build_timeline_rows(trace: &Trace) -> Vec<TimelineRow> {
    let mut rows = trace
        .instructions
        .iter()
        .map(|instruction| TimelineRow {
            inst_id: instruction.inst_id,
            label: instruction_label(instruction),
            spans: Vec::new(),
            last_cycle: None,
        })
        .collect::<Vec<_>>();
    let row_indexes = rows
        .iter()
        .enumerate()
        .map(|(index, row)| (row.inst_id, index))
        .collect::<HashMap<_, _>>();

    for span in &trace.spans {
        let Some(row_index) = row_indexes.get(&span.inst_id).copied() else {
            continue;
        };
        if let Some(cycle) = span.cycle.checked_add(span.duration - 1) {
            rows[row_index].last_cycle = Some(
                rows[row_index]
                    .last_cycle
                    .map_or(cycle, |last| last.max(cycle)),
            );
        }
        insert_timeline_span(
            &mut rows[row_index].spans,
            TimelineSpan {
                cycle: span.cycle,
                duration: span.duration,
                cell: timeline_cell(&span.stage, &span.lane),
            },
        );
    }

    rows.sort_by_key(|row| row.inst_id);
    for row in &mut rows {
        row.spans.sort_by_key(|span| span.cycle);
    }
    rows
}

pub fn build_timeline_rows_fast(trace: &Trace) -> Vec<TimelineRow> {
    let mut rows = trace
        .instructions
        .iter()
        .map(|instruction| TimelineRow {
            inst_id: instruction.inst_id,
            label: instruction_label(instruction),
            spans: Vec::new(),
            last_cycle: None,
        })
        .collect::<Vec<_>>();
    let row_indexes = rows
        .iter()
        .enumerate()
        .map(|(index, row)| (row.inst_id, index))
        .collect::<HashMap<_, _>>();

    for span in &trace.spans {
        let Some(row_index) = row_indexes.get(&span.inst_id).copied() else {
            continue;
        };
        if let Some(cycle) = span.cycle.checked_add(span.duration - 1) {
            rows[row_index].last_cycle = Some(
                rows[row_index]
                    .last_cycle
                    .map_or(cycle, |last| last.max(cycle)),
            );
        }
        rows[row_index].spans.push(TimelineSpan {
            cycle: span.cycle,
            duration: span.duration,
            cell: timeline_cell(&span.stage, &span.lane),
        });
    }

    rows.sort_by_key(|row| row.inst_id);
    for row in &mut rows {
        row.spans.sort_by_key(|span| span.cycle);
    }
    rows
}

impl TraceView {
    pub fn new(trace: &Trace) -> Self {
        let order = InstructionOrder::new(&trace.instructions);
        let row_count = order.len();
        let mut rows = (0..row_count)
            .map(|rank| {
                let instruction_index = order.instruction_index(rank);
                TimelineRowView {
                    inst_id: trace.instructions[instruction_index].inst_id,
                    instruction_index,
                    span_start: 0,
                    span_end: 0,
                    last_cycle: None,
                }
            })
            .collect::<Vec<_>>();

        let span_rows = trace.spans.iter().map(|span| order.rank(span.inst_id));
        let (span_offsets, mut span_order) = group_by_row(row_count, span_rows);
        for (rank, row) in rows.iter_mut().enumerate() {
            row.span_start = span_offsets[rank];
            row.span_end = span_offsets[rank + 1];
            let indexes = &mut span_order[row.span_start as usize..row.span_end as usize];
            if !indexes.is_sorted_by_key(|&index| trace.spans[index as usize].cycle) {
                indexes.sort_by_key(|&index| trace.spans[index as usize].cycle);
            }
            row.last_cycle = indexes
                .iter()
                .filter_map(|&index| {
                    let span = &trace.spans[index as usize];
                    span.cycle.checked_add(span.duration - 1)
                })
                .max();
        }

        let event_rows = trace.events.iter().map(|event| order.rank(event.inst_id));
        let (event_offsets, event_order) = group_by_row(row_count, event_rows);

        let mut retire_of_row = vec![NO_RECORD; row_count];
        for (retire_index, retire) in trace.retires.iter().enumerate() {
            if let Some(rank) = order.rank(retire.inst_id) {
                retire_of_row[rank] = index_u32(retire_index);
            }
        }

        Self {
            order,
            rows,
            span_order,
            event_offsets,
            event_order,
            retire_of_row,
        }
    }

    pub fn rows(&self) -> &[TimelineRowView] {
        &self.rows
    }

    pub fn row(&self, inst_id: u64) -> Option<&TimelineRowView> {
        self.order.rank(inst_id).map(|rank| &self.rows[rank])
    }

    fn row_spans<'t>(
        &self,
        row: &TimelineRowView,
        trace: &'t Trace,
    ) -> impl Iterator<Item = &'t ModelSpan> + use<'_, 't> {
        self.span_order[row.span_start as usize..row.span_end as usize]
            .iter()
            .map(|&index| &trace.spans[index as usize])
    }

    pub fn instruction_detail(&self, trace: &Trace, inst_id: u64) -> Option<InstructionDetail> {
        let rank = self.order.rank(inst_id)?;
        let row = &self.rows[rank];
        let instruction = trace.instructions.get(row.instruction_index)?;

        let mut spans = self
            .row_spans(row, trace)
            .map(span_detail)
            .collect::<Vec<_>>();
        sort_span_details(&mut spans);

        let event_indexes = &self.event_order
            [self.event_offsets[rank] as usize..self.event_offsets[rank + 1] as usize];
        let mut events = event_indexes
            .iter()
            .map(|&event_index| {
                let event = &trace.events[event_index as usize];
                EventDetail {
                    cycle: event.cycle,
                    event: event.event.clone(),
                    attrs: event.attrs.clone(),
                }
            })
            .collect::<Vec<_>>();
        events.sort_by_key(|event| event.cycle);

        let retire = self.retire_of_row[rank];
        Some(InstructionDetail {
            inst_id,
            label: instruction_label(instruction),
            attrs: instruction.attrs.clone(),
            spans,
            events,
            retire: (retire != NO_RECORD).then(|| retire_detail(&trace.retires[retire as usize])),
        })
    }
}

/// Buckets record indexes by row: `order[offsets[r]..offsets[r + 1]]` holds
/// row `r`'s records in their original order. Records without a row are
/// dropped.
fn group_by_row(
    row_count: usize,
    record_rows: impl Iterator<Item = Option<usize>> + Clone,
) -> (Vec<u32>, Vec<u32>) {
    let mut offsets = vec![0u32; row_count + 1];
    for rank in record_rows.clone().flatten() {
        offsets[rank + 1] += 1;
    }
    for rank in 0..row_count {
        offsets[rank + 1] += offsets[rank];
    }

    let mut cursor = offsets[..row_count].to_vec();
    let mut order = vec![0u32; offsets[row_count] as usize];
    for (record_index, rank) in record_rows.enumerate() {
        if let Some(rank) = rank {
            order[cursor[rank] as usize] = index_u32(record_index);
            cursor[rank] += 1;
        }
    }
    (offsets, order)
}

fn index_u32(index: usize) -> u32 {
    u32::try_from(index).expect("trace record count fits in u32")
}

pub fn timeline_cell_at(row: &TimelineRow, cycle: u64) -> Option<&TimelineCell> {
    row.spans
        .iter()
        .find(|span| cycle >= span.cycle && cycle < span.cycle.saturating_add(span.duration))
        .map(|span| &span.cell)
}

fn insert_timeline_span(spans: &mut Vec<TimelineSpan>, span: TimelineSpan) {
    let span_end = span.cycle.saturating_add(span.duration);

    if let Some(last) = spans.last_mut() {
        let last_end = last.cycle.saturating_add(last.duration);
        if last_end <= span.cycle {
            if last_end == span.cycle && last.cell == span.cell {
                last.duration = last.duration.saturating_add(span.duration);
            } else {
                spans.push(span);
            }
            return;
        }
    } else {
        spans.push(span);
        return;
    }

    let mut next = Vec::with_capacity(spans.len() + 1);

    for existing in spans.drain(..) {
        let existing_end = existing.cycle.saturating_add(existing.duration);
        if existing_end <= span.cycle || existing.cycle >= span_end {
            next.push(existing);
            continue;
        }

        if existing.cycle < span.cycle {
            next.push(TimelineSpan {
                cycle: existing.cycle,
                duration: span.cycle - existing.cycle,
                cell: existing.cell.clone(),
            });
        }

        if existing_end > span_end {
            next.push(TimelineSpan {
                cycle: span_end,
                duration: existing_end - span_end,
                cell: existing.cell,
            });
        }
    }

    next.push(span);
    next.sort_by_key(|span| span.cycle);
    merge_timeline_spans(next, spans);
}

fn merge_timeline_spans(input: Vec<TimelineSpan>, output: &mut Vec<TimelineSpan>) {
    for span in input {
        if let Some(last) = output.last_mut()
            && last.cell == span.cell
            && last.cycle.saturating_add(last.duration) == span.cycle
        {
            last.duration = last.duration.saturating_add(span.duration);
            continue;
        }
        output.push(span);
    }
}

pub fn build_instruction_details(trace: &Trace) -> BTreeMap<u64, InstructionDetail> {
    let mut details = trace
        .instructions
        .iter()
        .map(|instruction| {
            (
                instruction.inst_id,
                InstructionDetail {
                    inst_id: instruction.inst_id,
                    label: instruction_label(instruction),
                    attrs: instruction.attrs.clone(),
                    spans: Vec::new(),
                    events: Vec::new(),
                    retire: None,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();

    for span in &trace.spans {
        if let Some(detail) = details.get_mut(&span.inst_id) {
            detail.spans.push(span_detail(span));
        }
    }

    for event in &trace.events {
        if let Some(detail) = details.get_mut(&event.inst_id) {
            detail.events.push(EventDetail {
                cycle: event.cycle,
                event: event.event.clone(),
                attrs: event.attrs.clone(),
            });
        }
    }

    for retire in &trace.retires {
        if let Some(detail) = details.get_mut(&retire.inst_id) {
            detail.retire = Some(retire_detail(retire));
        }
    }

    for detail in details.values_mut() {
        sort_span_details(&mut detail.spans);
        detail.events.sort_by_key(|event| event.cycle);
    }

    details
}

pub fn build_instruction_detail(trace: &Trace, inst_id: u64) -> Option<InstructionDetail> {
    TraceView::new(trace).instruction_detail(trace, inst_id)
}

fn sort_span_details(spans: &mut [SpanDetail]) {
    spans.sort_by(|left, right| {
        left.cycle
            .cmp(&right.cycle)
            .then_with(|| left.stage.cmp(&right.stage))
            .then_with(|| left.lane.cmp(&right.lane))
    });
}

fn span_detail(span: &ModelSpan) -> SpanDetail {
    SpanDetail {
        cycle: span.cycle,
        duration: span.duration,
        stage: span.stage.to_string(),
        lane: span.lane.to_string(),
        attrs: span.attrs.clone(),
    }
}

fn retire_detail(retire: &RetireEvent) -> RetireDetail {
    RetireDetail {
        cycle: retire.cycle,
        status: retire.status.clone(),
        attrs: retire.attrs.clone(),
    }
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

/// Width of the widest `instruction_label`, measured without formatting.
fn max_label_width(trace: &Trace) -> usize {
    trace
        .instructions
        .iter()
        .map(|instruction| {
            let digits = instruction.inst_id.checked_ilog10().unwrap_or(0) as usize + 1;
            let attr_width = |key| {
                attr_value(&instruction.attrs, key).map_or(0, |value| value.chars().count() + 1)
            };
            1 + digits + attr_width("pc") + attr_width("asm")
        })
        .max()
        .unwrap_or(0)
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::analysis::SummaryOptions;
    use crate::parser::parse_plog;

    use super::{App, ColorMode, Overlay, Theme};

    #[test]
    fn full_trace_preserves_preview_view_state() {
        let path = Path::new("trace.plog");
        let options = SummaryOptions::default();
        let mut preview = App::new(
            path,
            parse_plog(concat!(
                "PLOG\t1\n",
                "STAGE\tIF\tFetch\n",
                "STAGE\tID\tDecode\n",
                "LANE\tmain\tMain\n",
                "I\t1\tpc=0x1000\n",
                "I\t2\tpc=0x1004\n",
                "B\t10\t1\t1\tmain\tIF\n",
                "B\t42\t1\t2\tmain\tID\n",
            ))
            .expect("preview trace parses"),
            Theme::new(ColorMode::Default),
            options,
        );
        preview.move_down();
        preview.cycle_offset = 42;
        preview.cell_width = 11;
        preview.overlay = Overlay::Detail;
        preview.jump_input = "2,42".to_owned();
        preview.ensure_selected_detail();

        let mut full = App::new(
            path,
            parse_plog(concat!(
                "PLOG\t1\n",
                "STAGE\tIF\tFetch\n",
                "STAGE\tID\tDecode\n",
                "STAGE\tEX\tExecute\n",
                "LANE\tmain\tMain\n",
                "I\t1\tpc=0x1000\n",
                "I\t2\tpc=0x1004\n",
                "I\t3\tpc=0x1008\n",
                "B\t10\t1\t1\tmain\tIF\n",
                "B\t42\t1\t2\tmain\tID\n",
                "B\t44\t1\t2\tmain\tEX\n",
                "B\t50\t1\t3\tmain\tIF\n",
            ))
            .expect("full trace parses"),
            Theme::new(ColorMode::Default),
            options,
        );

        full.preserve_view_state_from(&preview);

        assert_eq!(full.selected_row, 1);
        assert_eq!(full.cycle_offset, 42);
        assert_eq!(full.cell_width, 11);
        assert_eq!(full.overlay, Overlay::Detail);
        assert_eq!(full.jump_input, "2,42");
        assert_eq!(full.selected_detail().expect("detail preserved").inst_id, 2);
    }
}
