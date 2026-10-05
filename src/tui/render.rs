use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Clear, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use super::{App, InstructionDetail, Overlay, TimelineRowView, Viewport, attr_value};
use crate::analysis::{CountEntry, SpanStats};
use crate::model::{Instruction, KeyValue, Span as ModelSpan};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const MAX_DETAIL_SPANS: usize = 12;
const MAX_DETAIL_EVENTS: usize = 6;

/// Chrome styles: plain text with bold for emphasis and gray for secondary
/// information. Only the pipeline blocks carry color.
struct Palette {
    strong: Style,
    muted: Style,
    selected: Style,
}

impl Palette {
    fn new(colored: bool) -> Self {
        Self {
            strong: Style::new().bold(),
            muted: if colored {
                Style::new().fg(Color::DarkGray)
            } else {
                Style::new().dim()
            },
            selected: Style::new().reversed(),
        }
    }
}

pub(super) fn render(frame: &mut Frame<'_>, app: &App) {
    let palette = Palette::new(app.theme.colored());
    let [top, body, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    render_top_bar(frame, top, app, &palette);
    render_timeline(frame, body, app, &palette);
    render_status(frame, status, app, &palette);
    render_overlay(frame, frame.area(), app, &palette);
}

pub(super) fn render_loading(frame: &mut Frame<'_>, path: &Path, elapsed: Duration, colored: bool) {
    let palette = Palette::new(colored);
    let spinner = SPINNER[(elapsed.as_millis() / 100) as usize % SPINNER.len()];
    let lines = vec![
        Line::from(vec![
            Span::raw(format!("{spinner} loading ")),
            Span::styled(path.display().to_string(), palette.strong),
        ]),
        Line::from(vec![
            Span::styled("elapsed ", palette.muted),
            Span::raw(format!("{:.1}s", elapsed.as_secs_f32())),
        ]),
        Line::default(),
        Line::from(vec![
            Span::styled("Esc", palette.strong),
            Span::styled(" cancel", palette.muted),
        ]),
    ];
    let area = centered_rect(frame.area(), 72, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(panel("Loading", &palette)),
        area,
    );
}

fn render_top_bar(frame: &mut Frame<'_>, area: Rect, app: &App, palette: &Palette) {
    let mut left = vec![Span::styled(format!(" {}", app.file_name), palette.strong)];
    if app.preview {
        left.push(Span::styled("  (preview)", palette.muted));
    }

    let summary = &app.summary;
    let ipc = summary
        .ipc
        .map_or_else(|| "n/a".to_owned(), |ipc| format!("{ipc:.3}"));
    let cycles = match (summary.cycle_start, summary.cycle_end) {
        (Some(start), Some(end)) => format!("{}–{}", group(start), group(end)),
        _ => "n/a".to_owned(),
    };
    let mut right = Vec::new();
    for (key, value) in [
        ("inst", group(summary.instruction_count as u64)),
        ("retired", group(summary.retired_count as u64)),
        ("IPC", ipc),
        ("cycles", cycles),
    ] {
        right.push(Span::styled(format!("   {key} "), palette.muted));
        right.push(Span::raw(value));
    }
    right.push(Span::raw(" "));

    render_split_line(frame, area, Line::from(left), Line::from(right));
}

fn render_timeline(frame: &mut Frame<'_>, area: Rect, app: &App, palette: &Palette) {
    let row_count = app.row_count();
    let position = format!(
        " row {}/{}  cycle {}  zoom {} ",
        group((app.selected_row + 1).min(row_count) as u64),
        group(row_count as u64),
        group(app.cycle_offset),
        app.cell_width
    );
    let block = Block::bordered()
        .border_style(palette.muted)
        .title(Line::styled(" Timeline ", palette.strong))
        .title(Line::styled(position, palette.muted).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 2 || inner.width < 16 {
        return;
    }

    let row_limit = (inner.height - 1) as usize;
    let first_row = scroll_to_selection(app, row_limit);
    let label_width = (app.label_width + 1).clamp(8, (inner.width as usize / 3).max(8));
    let cell_width = app.cell_width as usize;
    let grid_width = (inner.width as usize).saturating_sub(label_width + 1);
    let visible_cycles = (grid_width / cell_width).max(1) as u64;
    app.viewport.set(Viewport {
        rows: row_limit,
        cycles: visible_cycles,
    });

    let rows = &app.view.rows()[first_row..(first_row + row_limit).min(row_count)];
    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push(Line::from(vec![
        Span::styled(fit_left("inst", label_width), palette.muted),
        Span::styled("│", palette.muted),
        Span::styled(
            cycle_ticks(app.cycle_offset, visible_cycles, cell_width),
            palette.muted,
        ),
    ]));
    for (index, row) in rows.iter().enumerate() {
        let selected = first_row + index == app.selected_row;
        lines.push(timeline_row(
            app,
            row,
            selected,
            label_width,
            visible_cycles,
            palette,
        ));
    }
    frame.render_widget(Paragraph::new(lines), inner);

    if row_count > row_limit {
        let mut state = ScrollbarState::new(row_count).position(app.selected_row);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(None)
                .thumb_style(palette.muted),
            area.inner(ratatui::layout::Margin {
                horizontal: 0,
                vertical: 1,
            }),
            &mut state,
        );
    }
}

/// Keeps the selection inside the viewport, scrolling only when it would
/// leave it, and never leaves blank rows below the last instruction.
fn scroll_to_selection(app: &App, row_limit: usize) -> usize {
    let mut first = app.row_offset.get();
    if app.selected_row < first {
        first = app.selected_row;
    } else if app.selected_row >= first + row_limit {
        first = app.selected_row + 1 - row_limit;
    }
    first = first.min(app.row_count().saturating_sub(row_limit));
    app.row_offset.set(first);
    first
}

fn timeline_row(
    app: &App,
    row: &TimelineRowView,
    selected: bool,
    label_width: usize,
    visible_cycles: u64,
    palette: &Palette,
) -> Line<'static> {
    let cell_width = app.cell_width as usize;
    let instruction = &app.trace.instructions[row.instruction_index];
    let mut spans = Vec::with_capacity(16);
    label_spans(instruction, label_width, selected, palette, &mut spans);
    spans.push(Span::styled("│", palette.muted));

    for run in row_runs(app, row, visible_cycles) {
        let width = run.width as usize * cell_width;
        match run.span {
            Some(span) => spans.push(Span::styled(
                fit_center(&block_label(span, width), width),
                app.theme.style_for_stage(&span.stage),
            )),
            None => spans.push(Span::raw(" ".repeat(width))),
        }
    }
    Line::from(spans)
}

fn label_spans(
    instruction: &Instruction,
    width: usize,
    selected: bool,
    palette: &Palette,
    spans: &mut Vec<Span<'static>>,
) {
    let pieces = [
        (Some(format!("#{}", instruction.inst_id)), palette.muted),
        (
            attr_value(&instruction.attrs, "pc").map(|pc| format!(" {pc}")),
            Style::new(),
        ),
        (
            attr_value(&instruction.attrs, "asm").map(|asm| format!(" {asm}")),
            Style::new(),
        ),
    ];
    let mut remaining = width;
    for (text, style) in pieces {
        let Some(text) = text else { continue };
        if remaining == 0 {
            break;
        }
        let text = truncate_chars(&text, remaining);
        remaining -= text.chars().count();
        spans.push(Span::styled(
            text,
            if selected { palette.selected } else { style },
        ));
    }
    if remaining > 0 {
        spans.push(Span::styled(
            " ".repeat(remaining),
            if selected {
                palette.selected
            } else {
                Style::new()
            },
        ));
    }
}

struct Run<'t> {
    width: u64,
    span: Option<&'t ModelSpan>,
}

/// Splits the visible window of a row into stage blocks and gaps, merging
/// neighbors that would render identically.
fn row_runs<'t>(app: &'t App, row: &TimelineRowView, visible_cycles: u64) -> Vec<Run<'t>> {
    let window_start = app.cycle_offset;
    let window_end = window_start.saturating_add(visible_cycles);
    let mut runs: Vec<Run<'t>> = Vec::new();
    let mut push = |width: u64, span: Option<&'t ModelSpan>| {
        if width == 0 {
            return;
        }
        if let Some(last) = runs.last_mut()
            && same_cell(last.span, span)
        {
            last.width += width;
            return;
        }
        runs.push(Run { width, span });
    };

    let mut cursor = window_start;
    for span in app.view.row_spans(row, &app.trace) {
        let span_end = span.cycle.saturating_add(span.duration);
        if span_end <= cursor {
            continue;
        }
        if span.cycle >= window_end {
            break;
        }
        if span.cycle > cursor {
            push(span.cycle - cursor, None);
            cursor = span.cycle;
        }
        let clipped_end = span_end.min(window_end);
        push(clipped_end - cursor, Some(span));
        cursor = clipped_end;
        if cursor >= window_end {
            break;
        }
    }
    push(window_end - cursor, None);
    runs
}

fn same_cell(left: Option<&ModelSpan>, right: Option<&ModelSpan>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => left.stage == right.stage && left.lane == right.lane,
        _ => false,
    }
}

/// `stage/lane` when it fits, otherwise just the stage.
fn block_label(span: &ModelSpan, width: usize) -> String {
    if span.lane == "main" {
        return span.stage.to_string();
    }
    let full = format!("{}/{}", span.stage, span.lane);
    if full.chars().count() <= width {
        full
    } else {
        span.stage.to_string()
    }
}

/// Cycle numbers spaced so that none overlap: every cycle when the cell is
/// wide enough, otherwise every 2, 5, 10, 20, 50, ... cycles.
fn cycle_ticks(offset: u64, cycles: u64, cell_width: usize) -> String {
    let width = cycles as usize * cell_width;
    let last = offset.saturating_add(cycles - 1);
    let digits = last.to_string().len();
    let step = tick_step(digits, cell_width);
    let mut buffer = vec![b' '; width];
    let mut cycle = offset.next_multiple_of(step);
    while cycle <= last {
        let text = cycle.to_string();
        let column = (cycle - offset) as usize * cell_width;
        let start = if step == 1 {
            column + cell_width.saturating_sub(text.len()) / 2
        } else {
            column
        };
        let end = start + text.len();
        if end > width {
            break;
        }
        buffer[start..end].copy_from_slice(text.as_bytes());
        let Some(next) = cycle.checked_add(step) else {
            break;
        };
        cycle = next;
    }
    String::from_utf8(buffer).expect("ticks are ASCII")
}

fn tick_step(digits: usize, cell_width: usize) -> u64 {
    if cell_width > digits {
        return 1;
    }
    let mut magnitude = 1u64;
    loop {
        for factor in [2, 5, 10] {
            let step = magnitude * factor;
            if step as usize * cell_width > digits {
                return step;
            }
        }
        magnitude *= 10;
    }
}

fn render_status(frame: &mut Frame<'_>, area: Rect, app: &App, palette: &Palette) {
    if app.overlay == Overlay::Jump {
        let line = Line::from(vec![
            Span::styled(" jump to row,cycle: ", palette.strong),
            Span::raw(app.jump_input.clone()),
            Span::raw("_"),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    let mut hints = Vec::new();
    for (key, action) in [
        ("?", "help"),
        ("↑↓←→", "move"),
        ("g", "jump"),
        ("i", "info"),
        ("d", "detail"),
        ("+/-", "zoom"),
        ("q", "quit"),
    ] {
        hints.push(Span::styled(format!(" {key}"), palette.strong));
        hints.push(Span::styled(format!(" {action}  "), palette.muted));
    }
    let message = Line::raw(format!("{} ", app.status));
    render_split_line(frame, area, Line::from(hints), message);
}

/// Draws `left` and, if it fits beside it, `right` against the right edge.
fn render_split_line(frame: &mut Frame<'_>, area: Rect, left: Line<'_>, right: Line<'_>) {
    let left_width = left.width() as u16;
    frame.render_widget(Paragraph::new(left), area);
    let right_width = right.width() as u16;
    if left_width + right_width < area.width {
        frame.render_widget(Paragraph::new(right.right_aligned()), area);
    }
}

fn render_overlay(frame: &mut Frame<'_>, area: Rect, app: &App, palette: &Palette) {
    match app.overlay {
        Overlay::None | Overlay::Jump => {}
        Overlay::Info => {
            let lines = info_lines(app, palette);
            show_panel(frame, area, "Info", lines, 96, palette);
        }
        Overlay::Detail => {
            let (title, lines) = match app.selected_detail() {
                Some(detail) => (
                    format!("Instruction #{}", detail.inst_id),
                    detail_lines(detail, palette),
                ),
                None => (
                    "Instruction".to_owned(),
                    vec![Line::styled("no instruction selected", palette.muted)],
                ),
            };
            show_panel(frame, area, &title, lines, 100, palette);
        }
        Overlay::Help => {
            let lines = help_lines(palette);
            show_panel(frame, area, "Keys", lines, 64, palette);
        }
    }
}

fn show_panel(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    lines: Vec<Line<'static>>,
    max_width: u16,
    palette: &Palette,
) {
    let content_width = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let width = (content_width + 4).clamp(32, max_width);
    let rect = centered_rect(area, width, lines.len() as u16 + 2);
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines).block(panel(title, palette)), rect);
}

fn panel<'a>(title: &str, palette: &Palette) -> Block<'a> {
    Block::bordered()
        .title(Line::styled(format!(" {title} "), palette.strong))
        .padding(Padding::horizontal(1))
}

const KEY_WIDTH: usize = 10;

fn key_value(key: &str, value: Vec<Span<'static>>, palette: &Palette) -> Line<'static> {
    let mut spans = vec![Span::styled(fit_left(key, KEY_WIDTH), palette.muted)];
    spans.extend(value);
    Line::from(spans)
}

fn section(title: &str, palette: &Palette) -> Line<'static> {
    Line::styled(title.to_owned(), palette.strong)
}

fn info_lines(app: &App, palette: &Palette) -> Vec<Line<'static>> {
    let summary = &app.summary;
    let mut lines = Vec::new();

    if let Some(row) = app.selected_row() {
        lines.push(key_value(
            "selected",
            vec![Span::styled(row.label(&app.trace), palette.strong)],
            palette,
        ));
        let at_cycle = app
            .view
            .row_spans(row, &app.trace)
            .find(|span| {
                app.cycle_offset >= span.cycle
                    && app.cycle_offset < span.cycle.saturating_add(span.duration)
            })
            .map_or_else(
                || vec![Span::styled("idle", palette.muted)],
                |span| {
                    vec![
                        Span::styled(span.stage.to_string(), palette.strong),
                        Span::styled(format!("  lane {}", span.lane), palette.muted),
                    ]
                },
            );
        lines.push(key_value(
            &format!("cycle {}", group(app.cycle_offset)),
            at_cycle,
            palette,
        ));
    }

    lines.push(Line::default());
    lines.push(section("Trace", palette));
    let ipc = summary
        .ipc
        .map_or_else(|| "n/a".to_owned(), |ipc| format!("{ipc:.3}"));
    for (key, value) in [
        ("inst", group(summary.instruction_count as u64)),
        ("retired", group(summary.retired_count as u64)),
        ("spans", group(summary.span_count as u64)),
        ("cycles", group(summary.cycle_count)),
        ("IPC", ipc),
    ] {
        lines.push(key_value(
            key,
            vec![Span::styled(value, palette.strong)],
            palette,
        ));
    }

    let has_stats = !summary.stage_stats.is_empty() || !summary.lane_stats.is_empty();
    if has_stats {
        lines.push(Line::default());
        lines.push(section("Occupancy", palette));
        span_stat_lines("stages", &summary.stage_stats, palette, &mut lines);
        span_stat_lines("lanes", &summary.lane_stats, palette, &mut lines);
    }

    lines.push(Line::default());
    lines.push(section("Bottlenecks", palette));
    lines.push(key_value(
        "top",
        count_entries(&summary.top_bottlenecks, palette),
        palette,
    ));
    for (key, counts) in [
        ("stalls", &summary.stall_reasons),
        ("flush", &summary.flush_reasons),
        ("replay", &summary.replay_reasons),
    ] {
        lines.push(key_value(key, map_counts(counts, palette), palette));
    }
    lines
}

fn span_stat_lines(
    label: &str,
    stats: &BTreeMap<String, SpanStats>,
    palette: &Palette,
    lines: &mut Vec<Line<'static>>,
) {
    if stats.is_empty() {
        return;
    }
    let mut value = Vec::new();
    for (key, stats) in stats.iter().take(6) {
        value.push(Span::styled(key.clone(), palette.strong));
        value.push(Span::styled(
            format!(
                " {} avg {:.1}  ",
                group(stats.total_cycles),
                stats.average_duration
            ),
            palette.muted,
        ));
    }
    lines.push(key_value(label, value, palette));
}

fn count_entries(entries: &[CountEntry], palette: &Palette) -> Vec<Span<'static>> {
    if entries.is_empty() {
        return vec![Span::styled("none", palette.muted)];
    }
    pairs(
        entries
            .iter()
            .take(3)
            .map(|entry| (entry.key.as_str(), entry.count)),
        palette,
    )
}

fn map_counts(counts: &BTreeMap<String, u64>, palette: &Palette) -> Vec<Span<'static>> {
    if counts.is_empty() {
        return vec![Span::styled("none", palette.muted)];
    }
    pairs(
        counts
            .iter()
            .take(4)
            .map(|(key, count)| (key.as_str(), *count)),
        palette,
    )
}

fn pairs<'a>(
    entries: impl Iterator<Item = (&'a str, u64)>,
    palette: &Palette,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (key, count) in entries {
        spans.push(Span::raw(key.to_owned()));
        spans.push(Span::styled(format!(" {}  ", group(count)), palette.muted));
    }
    spans
}

fn detail_lines(detail: &InstructionDetail, palette: &Palette) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::styled(detail.label.clone(), palette.strong),
        key_value("attrs", attrs(&detail.attrs, palette), palette),
    ];
    let retire = match &detail.retire {
        Some(retire) => {
            let mut value = vec![
                Span::styled(retire.status.clone(), palette.strong),
                Span::styled(
                    format!(" at cycle {}  ", group(retire.cycle)),
                    palette.muted,
                ),
            ];
            if !retire.attrs.is_empty() {
                value.extend(attrs(&retire.attrs, palette));
            }
            value
        }
        None => vec![Span::styled("not retired", palette.muted)],
    };
    lines.push(key_value("retire", retire, palette));

    lines.push(Line::default());
    lines.push(section(&format!("Spans ({})", detail.spans.len()), palette));
    if detail.spans.is_empty() {
        lines.push(Line::styled("none", palette.muted));
    }
    for span in detail.spans.iter().take(MAX_DETAIL_SPANS) {
        let mut spans = vec![
            Span::styled(format!("{:>10} ", group(span.cycle)), Style::new()),
            Span::styled(format!("+{:<4} ", span.duration), palette.muted),
            Span::styled(format!("{:<6}", span.stage), palette.strong),
            Span::raw(format!("{}  ", span.lane)),
        ];
        if !span.attrs.is_empty() {
            spans.extend(attrs(&span.attrs, palette));
        }
        lines.push(Line::from(spans));
    }
    more_line(detail.spans.len(), MAX_DETAIL_SPANS, palette, &mut lines);

    lines.push(Line::default());
    lines.push(section(
        &format!("Events ({})", detail.events.len()),
        palette,
    ));
    if detail.events.is_empty() {
        lines.push(Line::styled("none", palette.muted));
    }
    for event in detail.events.iter().take(MAX_DETAIL_EVENTS) {
        let mut spans = vec![
            Span::styled(format!("{:>10} ", group(event.cycle)), Style::new()),
            Span::styled(format!("{}  ", event.event), palette.strong),
        ];
        spans.extend(attrs(&event.attrs, palette));
        lines.push(Line::from(spans));
    }
    more_line(detail.events.len(), MAX_DETAIL_EVENTS, palette, &mut lines);
    lines
}

fn more_line(total: usize, shown: usize, palette: &Palette, lines: &mut Vec<Line<'static>>) {
    if total > shown {
        lines.push(Line::styled(
            format!("{:>10} … {} more", "", total - shown),
            palette.muted,
        ));
    }
}

fn attrs(attrs: &[KeyValue], palette: &Palette) -> Vec<Span<'static>> {
    if attrs.is_empty() {
        return vec![Span::styled("none", palette.muted)];
    }
    let mut spans = Vec::new();
    for attr in attrs.iter().take(6) {
        spans.push(Span::styled(format!("{}=", attr.key), palette.muted));
        spans.push(Span::raw(format!("{} ", attr.value)));
    }
    if attrs.len() > 6 {
        spans.push(Span::styled("…", palette.muted));
    }
    spans
}

fn help_lines(palette: &Palette) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (title, entries) in [
        (
            "Navigate",
            &[
                ("↑ ↓  k j", "previous / next instruction"),
                ("PgUp PgDn", "page through instructions"),
                ("← →  h l", "scroll one cycle"),
                ("⇧← ⇧→", "scroll one page of cycles"),
                ("Home End", "first / last cycle of selection"),
                ("g", "jump to row,cycle"),
            ][..],
        ),
        (
            "View",
            &[
                ("+ / -", "zoom in / out"),
                ("i", "trace info"),
                ("d", "instruction detail"),
                ("?", "this help"),
                ("Esc", "close panel, or quit"),
                ("q", "quit"),
            ][..],
        ),
        (
            "Mouse",
            &[
                ("wheel", "move selection"),
                ("Alt+wheel", "scroll cycles"),
                ("Ctrl+wheel", "zoom"),
            ][..],
        ),
    ] {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(section(title, palette));
        for (keys, action) in entries {
            lines.push(Line::from(vec![
                Span::styled(fit_left(keys, 12), palette.strong),
                Span::styled((*action).to_owned(), palette.muted),
            ]));
        }
    }
    lines
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

/// Formats an integer with thin thousands separators: 1234567 → 1,234,567.
fn group(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

fn fit_left(value: &str, width: usize) -> String {
    let truncated = truncate_chars(value, width);
    format!("{truncated:<width$}")
}

fn fit_center(value: &str, width: usize) -> String {
    let truncated = truncate_chars(value, width);
    let padding = width.saturating_sub(truncated.chars().count());
    let left = padding / 2;
    format!(
        "{}{}{}",
        " ".repeat(left),
        truncated,
        " ".repeat(padding - left)
    )
}

fn truncate_chars(value: &str, width: usize) -> String {
    value.chars().take(width).collect()
}

#[cfg(test)]
mod tests {
    use super::{cycle_ticks, group, tick_step};

    #[test]
    fn groups_thousands() {
        assert_eq!(group(0), "0");
        assert_eq!(group(999), "999");
        assert_eq!(group(1_000), "1,000");
        assert_eq!(group(1_234_567), "1,234,567");
    }

    #[test]
    fn ticks_label_every_cycle_when_cells_fit_the_numbers() {
        assert_eq!(tick_step(4, 5), 1);
        assert_eq!(tick_step(3, 3), 2);
        assert_eq!(cycle_ticks(9, 3, 5), "  9   10   11  ");
    }

    #[test]
    fn ticks_thin_out_instead_of_overlapping() {
        assert_eq!(tick_step(4, 1), 5);
        assert_eq!(tick_step(4, 2), 5);
        assert_eq!(tick_step(4, 3), 2);
        assert_eq!(cycle_ticks(1000, 12, 1), "1000 1005   ");
    }
}
