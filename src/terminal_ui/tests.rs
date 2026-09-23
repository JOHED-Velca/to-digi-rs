use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::time::Duration;

use super::app::TerminalApp;
use super::model::{
    AnalysisSummary, CANONICAL_MDB_DISPLAY_PATH, DeploymentSummary, DetailsContext,
    DiagnosticsSummaryView, DryRunSummaryView, ImportSummaryView, InputMode, ReadinessSummaryView,
    Screen, TerminalUiOptions, UiAction, UiOperation,
};
use super::render::{
    compact_references, render_app, render_full_import_progress_body, status_line,
};
use super::should_launch_interactive;
use crate::cli::CliCommand;
use crate::digiweb::preflight::{AuthenticationReadinessStatus, ReadinessResult};
use crate::import::runner::ImportProgressSnapshot;
use crate::selection::SelectionCriteria;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn render_to_string(app: &TerminalApp, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal.draw(|frame| render_app(frame, app)).expect("draw");
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>()
}

#[test]
fn no_argument_interactive_mode_requires_both_ttys() {
    assert!(should_launch_interactive(false, true, true));
    assert!(!should_launch_interactive(false, false, true));
    assert!(!should_launch_interactive(false, true, false));
    assert!(!should_launch_interactive(true, true, true));
}

#[test]
fn default_selection_is_guided_import_and_navigation_wraps() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    assert_eq!(app.visible_items()[app.selected_index], "Guided Import");
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.visible_items()[app.selected_index], "Analyze MDB");
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.visible_items()[app.selected_index], "Guided Import");
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.visible_items()[app.selected_index], "Exit");
}

#[test]
fn enter_selects_and_escape_returns_and_main_menu_ignores_quit_keys() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.screen, Screen::DeploymentCheck);
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.screen, Screen::Main);
    app.handle_key(key(KeyCode::Char('q')));
    assert!(!app.should_quit);
    app.handle_key(key(KeyCode::Esc));
    assert!(!app.should_quit);
    app.selected_index = 7;
    app.handle_key(key(KeyCode::Enter));
    assert!(app.should_quit);
}

#[test]
fn destructive_full_import_confirmation_defaults_to_cancel() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: Some("bigway".to_string()),
    });
    app.screen = Screen::FullImportConfirm;
    assert_eq!(app.visible_items()[app.selected_index], "Cancel");
    app.handle_key(key(KeyCode::Enter));
    assert!(app.pending_action.is_none());
    assert_eq!(app.screen, Screen::Main);
}

#[test]
fn reference_confirmation_defaults_to_no() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: Some("bigway".to_string()),
    });
    app.screen = Screen::References;
    assert_eq!(app.visible_items()[app.selected_index], "No");
    app.handle_key(key(KeyCode::Enter));
    assert!(app.pending_action.is_none());
}

#[test]
fn ui_actions_reuse_existing_cli_selection_semantics() {
    let cli = UiAction::DryRunFirst {
        profile: Some("bigway".to_string()),
    }
    .into_cli();
    match cli.command {
        Some(CliCommand::DryRun(args)) => {
            assert!(args.test);
            assert_eq!(args.plu, None);
            assert_eq!(args.profile.as_deref(), Some("bigway"));
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let cli = UiAction::ImportSpecific {
        profile: Some("starsky".to_string()),
        plu: 7575,
    }
    .into_cli();
    match cli.command {
        Some(CliCommand::Import(args)) => {
            assert_eq!(args.plu, Some(7575));
            assert!(!args.test);
            assert_eq!(args.profile.as_deref(), Some("starsky"));
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn compact_references_sort_dedup_and_pad_departments() {
    let text = compact_references(
        &[11, 1, 2, 1],
        &[(2, 5), (1, 997), (1, 2), (1, 2), (11, 4)],
        &[4, 1, 4],
        120,
    );
    assert!(text.contains("Departments: 1, 2, 11"));
    assert!(text.contains("Department 01: 2, 997"));
    assert!(text.contains("Department 02: 5"));
    assert!(text.contains("Department 11: 4"));
    assert!(text.contains("Label formats: 1, 4"));
    assert!(!text.contains("Department 0011"));
}

#[test]
fn compact_references_wrap_with_hanging_indent() {
    let groups = (1..=12).map(|group| (1, group)).collect::<Vec<_>>();
    let text = compact_references(&[1], &groups, &[1], 44);
    assert!(text.contains("Department 01:"));
    assert!(
        text.lines()
            .any(|line| line.starts_with("                 10"))
    );
}

#[test]
fn render_main_screen_contains_full_logo_and_selection() {
    let app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    let rendered = render_to_string(&app, 100, 30);
    assert!(rendered.contains("DIGI"));
    assert!(rendered.contains("████"));
    assert!(!rendered.contains("============"));
    assert!(rendered.contains("> Guided Import"));
    assert!(rendered.contains("DIGIweb Import Tool"));
    assert!(!rendered.contains("q quit"));
}

#[test]
fn render_narrow_screen_uses_compact_logo_without_panic() {
    let app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    let rendered = render_to_string(&app, 60, 20);
    assert!(rendered.contains("DIGI"));
    assert!(rendered.contains("> Guided Import"));
}

#[test]
fn very_small_terminal_does_not_panic() {
    let app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    let rendered = render_to_string(&app, 30, 8);
    assert!(rendered.contains("Terminal too small"));
}

#[test]
fn secret_values_are_never_rendered() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    app.screen = Screen::DigiwebConfig;
    app.start_input(InputMode::Secret, "super-secret-value");
    let rendered = render_to_string(&app, 100, 30);
    assert!(rendered.contains("********"));
    assert!(!rendered.contains("super-secret-value"));
    assert!(!format!("{app:?}").contains("super-secret-value"));
}

#[test]
fn digiweb_host_input_accepts_hostname_characters() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    app.start_input(InputMode::DigiwebHost, "");
    for ch in "vps-51d2863e.vps.ovh.ca".chars() {
        app.handle_key(key(KeyCode::Char(ch)));
    }
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(
        app.pending_operation,
        Some(UiOperation::SaveHost("vps-51d2863e.vps.ovh.ca".to_string()))
    );
}

#[test]
fn numeric_plu_input_accepts_digits_and_backspace() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: Some("bigway".to_string()),
    });
    app.start_input(InputMode::DryRunPlu, "");
    for ch in ['7', '5', '7', '4'] {
        app.handle_key(key(KeyCode::Char(ch)));
    }
    app.handle_key(key(KeyCode::Backspace));
    app.handle_key(key(KeyCode::Char('5')));
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.input_mode, None);
    assert_eq!(app.input_buffer, "");
    assert_eq!(
        app.pending_operation,
        Some(UiOperation::DryRun(SelectionCriteria {
            limit: None,
            requested_plu: Some(7575),
            test_mode: false,
        }))
    );
}

#[test]
fn plu_input_rejects_empty_zero_and_non_digits_without_fallback() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    app.start_input(InputMode::LivePlu, "");
    app.handle_key(key(KeyCode::Char('a')));
    assert_eq!(app.input_buffer, "");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.input_error.as_deref(), Some("PLU number is required"));
    assert!(app.pending_operation.is_none());

    app.input_buffer = "0".to_string();
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.input_error.as_deref(),
        Some("PLU number must be greater than zero")
    );
    assert!(app.pending_operation.is_none());
}

#[test]
fn escaped_input_clears_secret_without_operation() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    app.start_input(InputMode::Secret, "super-secret-value");
    app.handle_key(key(KeyCode::Esc));

    assert_eq!(app.input_mode, None);
    assert_eq!(app.input_buffer, "");
    assert!(app.pending_operation.is_none());
}

#[test]
fn analysis_summary_render_uses_actual_counts_and_references() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: Some("starsky".to_string()),
    });
    app.screen = Screen::Analysis;
    app.analysis = Some(AnalysisSummary {
        source_plus: 3481,
        eligible: 3447,
        excluded: 34,
        departments: vec![3, 1],
        groups: vec![(1, 997), (1, 2), (3, 1)],
        label_formats: vec![6, 1],
        warning_count: 2,
        blocking_count: 0,
        first_valid_plu: Some(7575),
    });

    let rendered = render_to_string(&app, 100, 30);
    assert!(rendered.contains("Source PLUs:  3481"));
    assert!(rendered.contains("Eligible:     3447"));
    assert!(rendered.contains("Excluded:     34"));
    assert!(rendered.contains("Departments: 1, 3"));
    assert!(rendered.contains("Department 01: 2, 997"));
    assert!(rendered.contains("Label formats: 1, 6"));
    assert!(!rendered.contains("997Label formats"));
}

#[test]
fn reference_layout_uses_separate_lines_at_common_terminal_widths() {
    for (width, height) in [(80, 24), (100, 30), (121, 30), (120, 40)] {
        let mut app = TerminalApp::new(TerminalUiOptions {
            default_profile: Some("starsky".to_string()),
        });
        app.screen = Screen::References;
        app.analysis = Some(AnalysisSummary {
            source_plus: 5,
            eligible: 4,
            excluded: 1,
            departments: vec![1, 2, 3],
            groups: vec![(1, 1), (1, 2), (1, 997), (2, 1), (2, 998), (3, 7)],
            label_formats: vec![21, 1, 6],
            warning_count: 0,
            blocking_count: 0,
            first_valid_plu: Some(1),
        });

        let rendered = render_to_string(&app, width, height);
        assert!(rendered.contains("Departments: 1, 2, 3"));
        assert!(rendered.contains("Groups:"));
        assert!(rendered.contains("Department 01:"));
        assert!(rendered.contains("Department 02:"));
        assert!(rendered.contains("Label formats:"));
        assert!(!rendered.contains("3Groups"));
        assert!(!rendered.contains("997Label"));
    }
}

#[test]
fn diagnostics_summary_does_not_dump_details_by_default() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    app.screen = Screen::Diagnostics;
    app.diagnostics = Some(DiagnosticsSummaryView {
        eligible: 3447,
        excluded: 34,
        missing_name_count: 5,
        duplicate_barcode_count: 29,
        problem_count: 34,
    });

    let rendered = render_to_string(&app, 100, 30);
    assert!(rendered.contains("Missing PLU name"));
    assert!(rendered.contains("Duplicate barcode"));
    assert!(rendered.contains("Eligible"));
    assert!(!rendered.contains("payload"));
    assert!(!rendered.contains("request_id"));
}

#[test]
fn full_import_progress_renders_live_snapshot_counts() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    app.screen = Screen::FullImportProgress;
    app.import_progress = Some(ImportProgressSnapshot {
        selected: 10,
        completed: 4,
        success: 3,
        failed: 1,
        unknown: 0,
        active: 2,
        remaining: 4,
        elapsed: Duration::from_secs(8),
    });

    let rendered = render_full_import_progress_body(&app)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("4/10 (40.0%)"));
    assert!(rendered.contains("Successful"));
    assert!(rendered.contains("3"));
    assert!(rendered.contains("Failed"));
    assert!(rendered.contains("1"));
    assert!(rendered.contains("Active 2"));
    assert!(rendered.contains("Remaining 4"));
    assert!(rendered.contains("00:08 elapsed"));
    assert!(!rendered.contains("73.7%"));
    assert!(!rendered.contains("2541"));
}

#[test]
fn not_ready_blocks_dry_run_continuation() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    app.screen = Screen::ReadinessReady;
    app.readiness = Some(ReadinessSummaryView {
        readiness: ReadinessResult::NotReady,
        authentication: AuthenticationReadinessStatus::Passed,
        authentication_error: None,
        eligible: 10,
        excluded: 1,
        unverified_references: 2,
    });

    assert_eq!(
        app.visible_items(),
        vec!["Run Readiness Check", "View Details", "Back"]
    );
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.pending_operation, Some(UiOperation::Verify));
    app.pending_operation = None;
    app.selected_index = 1;
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.screen, Screen::Details);
    assert_eq!(app.details_context, DetailsContext::Readiness);
}

#[test]
fn ready_with_skips_allows_dry_run_continuation() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    app.screen = Screen::ReadinessReady;
    app.readiness = Some(ReadinessSummaryView {
        readiness: ReadinessResult::ReadyWithSkips,
        authentication: AuthenticationReadinessStatus::Passed,
        authentication_error: None,
        eligible: 10,
        excluded: 1,
        unverified_references: 0,
    });

    assert_eq!(app.visible_items()[0], "Continue");
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.screen, Screen::DryRun);
}

#[test]
fn readiness_without_result_offers_run_check_not_review() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    app.screen = Screen::ReadinessReady;

    assert_eq!(app.visible_items(), vec!["Run Readiness Check", "Back"]);
    let rendered = render_to_string(&app, 100, 30);
    assert!(rendered.contains("No readiness check has been run."));
    assert!(!rendered.contains("Readiness has not run yet."));
}

#[test]
fn dry_run_actions_map_to_existing_selection_modes() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    app.screen = Screen::DryRun;
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.pending_operation,
        Some(UiOperation::DryRun(SelectionCriteria {
            limit: Some(1),
            requested_plu: None,
            test_mode: true,
        }))
    );

    app.pending_operation = None;
    app.selected_index = 2;
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.pending_operation,
        Some(UiOperation::DryRun(SelectionCriteria {
            limit: None,
            requested_plu: None,
            test_mode: false,
        }))
    );
}

#[test]
fn dry_run_result_continue_moves_to_single_live_test() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    app.screen = Screen::DryRun;
    app.dry_run = Some(DryRunSummaryView {
        selected: 1,
        would_submit: 1,
        api_write_requests: 0,
        selected_plu: Some(7575),
        eligible: 10,
        excluded: 1,
        success: true,
    });

    assert_eq!(
        app.visible_items(),
        vec!["Continue", "View Details", "Back"]
    );
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.screen, Screen::SinglePlu);
}

#[test]
fn single_live_completion_continue_moves_to_full_import_confirmation() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: Some("starsky".to_string()),
    });
    app.screen = Screen::Completion;
    app.live_selection = Some(SelectionCriteria {
        limit: Some(1),
        requested_plu: None,
        test_mode: true,
    });
    app.last_live_import_was_single = true;
    app.import_result = Some(ImportSummaryView {
        selected: 1,
        submitted: 1,
        succeeded: 1,
        failed: 0,
        unknown: 0,
        selected_plu: Some(7575),
        status: "SUCCESS".to_string(),
    });

    assert_eq!(
        app.visible_items(),
        vec!["Continue", "View Details", "Back"]
    );
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(app.screen, Screen::FullImportConfirm);
    assert_eq!(app.live_selection, None);
    assert!(!app.last_live_import_was_single);
    assert_eq!(app.visible_items(), vec!["Cancel", "Start Import"]);
}

#[test]
fn deployment_missing_mdb_has_concise_message() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    app.screen = Screen::DeploymentCheck;
    app.deployment = Some(DeploymentSummary {
        mdb_found: false,
        config_ready: true,
        profiles_ready: true,
    });
    let rendered = render_to_string(&app, 100, 30);

    assert!(rendered.contains("MDB not found"));
    assert!(rendered.contains(CANONICAL_MDB_DISPLAY_PATH));
    assert!(app.visible_items().contains(&"Check Again"));
}

#[test]
fn completion_summary_separates_failed_and_unknown() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    app.screen = Screen::Completion;
    app.import_result = Some(ImportSummaryView {
        selected: 3,
        submitted: 3,
        succeeded: 1,
        failed: 1,
        unknown: 1,
        selected_plu: None,
        status: "COMPLETED_WITH_ERRORS".to_string(),
    });
    let rendered = render_to_string(&app, 100, 30);

    assert!(rendered.contains("Import Completed With Issues"));
    assert!(rendered.contains("Successful"));
    assert!(rendered.contains("Failed"));
    assert!(rendered.contains("Unknown"));
}

#[test]
fn details_screens_render_structured_context_without_log_file_placeholder() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    app.screen = Screen::Details;
    app.details_context = DetailsContext::Readiness;
    app.readiness = Some(ReadinessSummaryView {
        readiness: ReadinessResult::NotReady,
        authentication: AuthenticationReadinessStatus::Failed,
        authentication_error: Some("DIGIweb returned HTTP 401 Unauthorized.".to_string()),
        eligible: 4,
        excluded: 1,
        unverified_references: 2,
    });
    app.analysis = Some(AnalysisSummary {
        departments: vec![1, 2],
        groups: vec![(1, 997), (2, 998)],
        label_formats: vec![21],
        ..AnalysisSummary::default()
    });

    let rendered = render_to_string(&app, 100, 30);
    assert!(rendered.contains("Readiness Details"));
    assert!(rendered.contains("HTTP 401 Unauthorized"));
    assert!(rendered.contains("Client ID:"));
    assert!(rendered.contains("digi"));
    assert!(rendered.contains("Missing references"));
    assert!(!rendered.contains("Detailed reports remain"));
}

#[test]
fn diagnostic_dry_run_and_import_details_are_structured() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    app.screen = Screen::Details;
    app.details_context = DetailsContext::Diagnostics;
    app.diagnostics = Some(DiagnosticsSummaryView {
        eligible: 3447,
        excluded: 34,
        missing_name_count: 5,
        duplicate_barcode_count: 29,
        problem_count: 34,
    });
    let diagnostics = render_to_string(&app, 100, 30);
    assert!(diagnostics.contains("Diagnostics Details"));
    assert!(diagnostics.contains("Missing PLU name:"));

    app.details_context = DetailsContext::DryRun;
    app.dry_run = Some(DryRunSummaryView {
        selected: 1,
        would_submit: 1,
        api_write_requests: 0,
        selected_plu: Some(7575),
        eligible: 10,
        excluded: 1,
        success: true,
    });
    let dry_run = render_to_string(&app, 100, 30);
    assert!(dry_run.contains("Dry-run Details"));
    assert!(dry_run.contains("No API writes performed."));

    app.details_context = DetailsContext::FullImport;
    app.import_result = Some(ImportSummaryView {
        selected: 3,
        submitted: 3,
        succeeded: 1,
        failed: 1,
        unknown: 1,
        selected_plu: Some(1234),
        status: "COMPLETED_WITH_ERRORS".to_string(),
    });
    let import = render_to_string(&app, 100, 30);
    assert!(import.contains("Full Import Details"));
    assert!(import.contains("Unknown:"));
}

#[test]
fn details_scroll_keys_change_scroll_without_leaving_screen() {
    let mut app = TerminalApp::new(TerminalUiOptions {
        default_profile: None,
    });
    app.screen = Screen::Details;
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.details_scroll, 1);
    assert_eq!(app.screen, Screen::Details);
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.details_scroll, 0);
}

#[test]
fn semantic_status_styles_remain_textual() {
    let ok = status_line(true, "Config", "Ready");
    let warn = status_line(false, "Unknown", "Review");
    let missing = status_line(false, "Config", "Missing");
    assert!(ok.to_string().contains("OK"));
    assert!(warn.to_string().contains("WARN"));
    assert!(missing.to_string().contains("WARN"));
    assert!(ok.spans[0].style.fg.is_some());
}
