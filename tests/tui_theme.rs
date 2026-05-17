use pipeview::parser::parse_plog;
use pipeview::tui::{
    ColorMode, Theme, build_instruction_details, build_timeline_rows, parse_jump_target,
    timeline_runs, visible_cycle_count,
};
use ratatui::style::{Color, Style};

const THEME_JSON: &str = include_str!("../examples/theme.json");

#[test]
fn timeline_rows_preserve_stage_and_lane_metadata() {
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
    assert_eq!(rows[0].cells[&1].stage, "IF");
    assert_eq!(rows[0].cells[&1].lane, "main");
    assert_eq!(rows[0].cells[&1].label, "IF");
    assert_eq!(rows[0].cells[&2].stage, "ID");
    assert_eq!(rows[0].cells[&2].lane, "stall");
    assert_eq!(rows[0].cells[&2].label, "ID/stall");
    assert_eq!(rows[0].cells[&3].label, "ID/stall");
}

#[test]
fn timeline_runs_merge_adjacent_matching_cells() {
    let trace = parse_plog(concat!(
        "PLOG\t1\n",
        "STAGE\tIF\tFetch\n",
        "STAGE\tID\tDecode\n",
        "STAGE\tEX\tExecute\n",
        "LANE\tmain\tMain\n",
        "LANE\tstall\tStall\n",
        "I\t1\n",
        "B\t1\t1\t1\tmain\tIF\n",
        "B\t2\t3\t1\tstall\tID\treason=load_use\n",
        "B\t5\t1\t1\tmain\tEX\n",
        "R\t6\t1\tretire\n",
    ))
    .expect("valid trace");
    let rows = build_timeline_rows(&trace);
    let runs = timeline_runs(&rows[0], 1, 5);

    assert_eq!(runs.len(), 3);
    assert_eq!(runs[0].width, 1);
    assert_eq!(runs[0].cell.as_ref().expect("IF run").label, "IF");
    assert_eq!(runs[1].width, 3);
    assert_eq!(runs[1].cell.as_ref().expect("stall run").label, "ID/stall");
    assert_eq!(runs[2].width, 1);
    assert_eq!(runs[2].cell.as_ref().expect("EX run").label, "EX");
}

#[test]
fn jump_target_parses_row_and_absolute_cycle() {
    assert_eq!(parse_jump_target("12,345"), Some((12, 345)));
    assert_eq!(parse_jump_target("12:345"), Some((12, 345)));
    assert_eq!(parse_jump_target("12 345"), Some((12, 345)));
    assert_eq!(parse_jump_target("0,345"), None);
    assert_eq!(parse_jump_target("12"), None);
    assert_eq!(parse_jump_target("12,345,6"), None);
}

#[test]
fn visible_cycle_count_tracks_zoom_cell_width() {
    assert_eq!(visible_cycle_count(80, 5), 12);
    assert_eq!(visible_cycle_count(80, 10), 6);
    assert_eq!(visible_cycle_count(10, 10), 1);
}

#[test]
fn instruction_details_include_attrs_spans_events_and_retire_status() {
    let trace = parse_plog(concat!(
        "PLOG\t1\n",
        "STAGE\tIF\tFetch\n",
        "STAGE\tID\tDecode\n",
        "LANE\tmain\tMain\n",
        "LANE\tstall\tStall\n",
        "I\t7\tpc=0x80000020\tasm=lw\top=load\n",
        "B\t9\t1\t7\tmain\tIF\n",
        "B\t11\t2\t7\tstall\tID\treason=load_use\tresource=rs1\n",
        "E\t12\t7\tstall\treason=load_use\n",
        "R\t14\t7\tretire\tcommit=0\n",
    ))
    .expect("valid trace");

    let details = build_instruction_details(&trace);
    let detail = details.get(&7).expect("instruction detail");

    assert_eq!(detail.label, "#7 0x80000020 lw");
    assert_eq!(detail.attrs.len(), 3);
    assert_eq!(detail.spans.len(), 2);
    assert_eq!(detail.spans[0].cycle, 9);
    assert_eq!(detail.spans[0].stage, "IF");
    assert_eq!(detail.spans[1].cycle, 11);
    assert_eq!(detail.spans[1].duration, 2);
    assert_eq!(detail.spans[1].lane, "stall");
    assert_eq!(detail.spans[1].attrs[0].key, "reason");
    assert_eq!(detail.events.len(), 1);
    assert_eq!(detail.events[0].event, "stall");
    assert_eq!(
        detail.retire.as_ref().expect("retire detail").status,
        "retire"
    );
}

#[test]
fn default_theme_uses_fill_blocks_with_white_text() {
    let trace = parse_trace_with_stages(&["IF", "ID"]);
    let theme = Theme::new(ColorMode::Default).with_default_stage_colors(&trace.stages);

    let if_style = theme.style_for_stage("IF");
    let id_style = theme.style_for_stage("ID");

    assert_ne!(if_style, Style::default());
    assert_ne!(id_style, Style::default());
    assert_ne!(if_style, id_style);
    assert_eq!(if_style.fg, Some(Color::White));
    assert!(if_style.bg.is_some());
    assert_eq!(id_style.fg, Some(Color::White));
    assert!(id_style.bg.is_some());
}

#[test]
fn example_theme_overrides_fill_and_foreground_colors() {
    let trace = parse_trace_with_stages(&["IF", "ID", "EX"]);
    let theme = Theme::from_json_str(THEME_JSON)
        .expect("example theme parses")
        .with_default_stage_colors(&trace.stages);

    assert_eq!(
        theme.style_for_stage("IF"),
        Style::default()
            .fg(Color::White)
            .bg(Color::Rgb(0xff, 0x88, 0x00))
    );
    assert_eq!(
        theme.style_for_stage("ID"),
        Style::default().fg(Color::Black).bg(Color::Cyan)
    );
    assert_eq!(
        theme.style_for_stage("EX"),
        Style::default().fg(Color::LightGreen)
    );
}

#[test]
fn no_color_theme_disables_stage_colors() {
    let trace = parse_trace_with_stages(&["IF"]);
    let theme = Theme::new(ColorMode::None).with_default_stage_colors(&trace.stages);

    assert_eq!(theme.style_for_stage("IF"), Style::default());
}

#[test]
fn default_theme_reuses_palette_when_stage_count_exceeds_palette() {
    let stage_ids = (0..13).map(|index| format!("S{index}")).collect::<Vec<_>>();
    let stage_refs = stage_ids.iter().map(String::as_str).collect::<Vec<_>>();
    let trace = parse_trace_with_stages(&stage_refs);
    let theme = Theme::new(ColorMode::Default).with_default_stage_colors(&trace.stages);

    assert_eq!(theme.style_for_stage("S0"), theme.style_for_stage("S12"));
}

#[test]
fn theme_json_rejects_invalid_color_names() {
    assert!(Theme::from_json_str(r#"{"stages":{"IF":{"bg":"not-a-color"}}}"#).is_err());
}

#[test]
fn theme_json_rejects_alpha_outside_unit_range() {
    assert!(Theme::from_json_str(r#"{"stages":{"IF":{"bg":"red","alpha":1.5}}}"#).is_err());
}

fn parse_trace_with_stages(stage_ids: &[&str]) -> pipeview::model::Trace {
    let input = std::iter::once("PLOG\t1".to_owned())
        .chain(
            stage_ids
                .iter()
                .map(|stage| format!("STAGE\t{stage}\t{stage}")),
        )
        .chain([
            "LANE\tmain\tMain".to_owned(),
            "I\t1".to_owned(),
            format!("B\t1\t1\t1\tmain\t{}", stage_ids[0]),
            "R\t2\t1\tretire".to_owned(),
        ])
        .collect::<Vec<_>>()
        .join("\n");

    parse_plog(&input).expect("valid trace")
}
