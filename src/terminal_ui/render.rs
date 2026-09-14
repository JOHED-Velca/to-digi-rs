use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap};

use crate::config::AppConfig;
use crate::digiweb::preflight::{AuthenticationReadinessStatus, ReadinessResult};

use super::app::TerminalApp;
use super::model::{CANONICAL_MDB_DISPLAY_PATH, InputMode, LogoMode, Screen};
pub(super) fn render_app(frame: &mut ratatui::Frame<'_>, app: &TerminalApp) {
    let area = frame.area();
    frame.render_widget(Clear, area);
    if area.width < 46 || area.height < 12 {
        let text = vec![
            Line::from("to-digi-rs"),
            Line::from("Terminal too small."),
            Line::from("Use 80x24 or larger."),
            Line::from("q quit"),
        ];
        frame.render_widget(
            Paragraph::new(text)
                .block(Block::default().borders(Borders::ALL))
                .alignment(Alignment::Center),
            area,
        );
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(if app.screen == Screen::Main { 8 } else { 3 }),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, chunks[0], app.screen, area.width);
    render_screen(frame, chunks[1], app);
    frame.render_widget(
        Paragraph::new("Up/Down move    Enter select    Esc back    q quit")
            .style(secondary_style())
            .alignment(Alignment::Center),
        chunks[2],
    );
}

fn render_header(frame: &mut ratatui::Frame<'_>, area: Rect, screen: Screen, width: u16) {
    let no_color = std::env::var_os("NO_COLOR").is_some();
    if screen == Screen::Main {
        let lines = logo_lines(if width < 74 {
            LogoMode::Compact
        } else {
            LogoMode::Full
        });
        let styled = lines
            .into_iter()
            .map(|line| {
                if no_color {
                    Line::from(line)
                } else {
                    Line::from(Span::styled(line, Style::default().fg(Color::Cyan)))
                }
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(styled)
                .block(Block::default().borders(Borders::ALL))
                .alignment(Alignment::Center),
            area,
        );
    } else {
        frame.render_widget(
            Paragraph::new(screen.title())
                .block(Block::default().borders(Borders::ALL))
                .alignment(Alignment::Center)
                .style(Style::default().add_modifier(Modifier::BOLD)),
            area,
        );
    }
}

fn render_screen(frame: &mut ratatui::Frame<'_>, area: Rect, app: &TerminalApp) {
    let body = if let Some(message) = &app.busy_message {
        vec![Line::from(message.clone())]
    } else if let Some(mode) = app.input_mode {
        render_input_body(app, mode)
    } else {
        match app.screen {
            Screen::Main => vec![
                Line::from("DIGIweb Import Tool"),
                Line::from(""),
                Line::from("Choose a workflow."),
            ],
            Screen::DeploymentCheck => render_deployment_body(app),
            Screen::DigiwebConfig => render_config_body(app),
            Screen::Profile => vec![
                Line::from(format!(
                    "Active profile: {}",
                    app.selected_profile.as_deref().unwrap_or("Generic")
                )),
                Line::from("Bigway - customer-specific nutrition and sanitization mappings."),
                Line::from("Starsky - customer-specific validation and field defaults."),
            ],
            Screen::Analysis => render_analysis_body(app, area.width),
            Screen::Diagnostics => render_diagnostics_body(app),
            Screen::References => vec![
                Line::from("Have these been checked in DIGIweb?"),
                Line::from(""),
                Line::from(reference_text_from_analysis(app, area.width)),
                Line::from(""),
                Line::from("No is selected by default."),
            ],
            Screen::ReadinessReady => render_readiness_body(app),
            Screen::DryRun => render_dry_run_body(app),
            Screen::SinglePlu => vec![Line::from(
                "Choose one live PLU test before considering a full import.",
            )],
            Screen::FullImportConfirm => render_full_import_confirm_body(app),
            Screen::FullImportProgress => render_full_import_progress_body(app),
            Screen::Completion => render_completion_body(app),
            Screen::Resume => vec![Line::from(
                "Resume is hidden from the main menu in this pass.",
            )],
            Screen::Advanced => vec![Line::from("No advanced TUI tools are enabled yet.")],
            Screen::Version => vec![
                Line::from(format!("to-digi-rs {}", env!("CARGO_PKG_VERSION"))),
                Line::from("No v0.10 tag is created by this UI."),
            ],
            Screen::Details => vec![Line::from(
                "Detailed reports remain in command output files.",
            )],
        }
    };

    let panels = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);

    frame.render_widget(
        Paragraph::new(body)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(app.screen.title()),
            )
            .wrap(Wrap { trim: false }),
        panels[0],
    );
    render_menu(frame, panels[1], app);

    if app.screen == Screen::FullImportProgress {
        let ratio = app
            .import_progress
            .as_ref()
            .map(|snapshot| (snapshot.percent() / 100.0).clamp(0.0, 1.0))
            .unwrap_or(0.0);
        let label = app
            .import_progress
            .as_ref()
            .map(|snapshot| format!("{:.1}%", snapshot.percent()))
            .unwrap_or_else(|| "0.0%".to_string());
        frame.render_widget(
            Gauge::default()
                .ratio(ratio)
                .style(Style::default().fg(Color::Cyan))
                .label(label),
            body_area(panels[0]),
        );
    }
}

fn render_input_body(app: &TerminalApp, mode: InputMode) -> Vec<Line<'static>> {
    let title = match mode {
        InputMode::ServerIp => "DIGIweb Server IP",
        InputMode::Secret => "Client Secret",
        InputMode::DryRunPlu | InputMode::LivePlu => "Specific PLU",
    };
    let prompt = match mode {
        InputMode::ServerIp => "Enter save    Esc cancel",
        InputMode::Secret => "Enter save    Esc cancel",
        InputMode::DryRunPlu | InputMode::LivePlu => "Enter run    Esc cancel",
    };
    let display_value = if mode == InputMode::Secret {
        if app.input_buffer.is_empty() {
            String::new()
        } else {
            "********".to_string()
        }
    } else {
        app.input_buffer.clone()
    };
    let mut lines = vec![
        Line::from(title.to_string()),
        Line::from(""),
        Line::from(format!("> {display_value}")),
        Line::from(""),
        Line::from(prompt.to_string()),
    ];
    if let Some(error) = &app.input_error {
        lines.push(Line::from(""));
        lines.push(Line::from(format!("ERROR {error}")));
    }
    lines
}

fn render_deployment_body(app: &TerminalApp) -> Vec<Line<'static>> {
    match &app.deployment {
        Some(summary) => {
            if summary.mdb_found {
                vec![
                    status_line(true, CANONICAL_MDB_DISPLAY_PATH, "Found"),
                    status_line(
                        summary.config_ready,
                        "Config",
                        if summary.config_ready {
                            "Ready"
                        } else {
                            "Missing"
                        },
                    ),
                    status_line(
                        summary.profiles_ready,
                        "Profiles",
                        if summary.profiles_ready {
                            "Ready"
                        } else {
                            "Missing"
                        },
                    ),
                ]
            } else {
                vec![
                    Line::from("MDB not found"),
                    Line::from(""),
                    Line::from("Expected:"),
                    Line::from(CANONICAL_MDB_DISPLAY_PATH),
                ]
            }
        }
        None => vec![Line::from("Press Check Again to inspect deployment.")],
    }
}

fn render_config_body(app: &TerminalApp) -> Vec<Line<'static>> {
    let config = AppConfig::load(Path::new("config.toml")).unwrap_or_default();
    let secret_configured = !config.digiweb.client_secret.trim().is_empty()
        && config.digiweb.client_secret.trim() != "CHANGE_ME";
    let mut lines = vec![
        Line::from("Server IP:"),
        Line::from(format!(
            "  {}",
            host_from_base_url(&config.digiweb.base_url)
        )),
        Line::from(""),
        Line::from("Client Secret:"),
        Line::from(if secret_configured {
            "  OK Configured".to_string()
        } else {
            "  WARN Not configured".to_string()
        }),
        Line::from(""),
        Line::from("Store:"),
        Line::from(format!("  {}", config.digiweb.store_number)),
    ];
    if let Some(message) = &app.status_message {
        lines.push(Line::from(""));
        lines.push(Line::from(message.clone()));
    }
    lines
}

fn host_from_base_url(value: &str) -> String {
    reqwest::Url::parse(value)
        .ok()
        .and_then(|url| url.host_str().map(ToString::to_string))
        .unwrap_or_else(|| value.to_string())
}

fn render_analysis_body(app: &TerminalApp, width: u16) -> Vec<Line<'static>> {
    let Some(summary) = &app.analysis else {
        return vec![Line::from("Analysis has not run yet.")];
    };
    let mut lines = vec![
        Line::from(format!("Source PLUs:  {}", summary.source_plus)),
        Line::from(format!("Eligible:     {}", summary.eligible)),
        Line::from(format!("Excluded:     {}", summary.excluded)),
        Line::from(""),
        Line::from(compact_references(
            &summary.departments,
            &summary.groups,
            &summary.label_formats,
            width.saturating_sub(8),
        )),
    ];
    if summary.excluded > 0 {
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "WARN {} records excluded",
            summary.excluded
        )));
    }
    lines
}

fn render_diagnostics_body(app: &TerminalApp) -> Vec<Line<'static>> {
    let Some(summary) = &app.diagnostics else {
        return vec![Line::from("Diagnostics have not run yet.")];
    };
    vec![
        status_line(
            summary.missing_name_count == 0,
            "Missing PLU name",
            &summary.missing_name_count.to_string(),
        ),
        status_line(
            summary.duplicate_barcode_count == 0,
            "Duplicate barcode",
            &summary.duplicate_barcode_count.to_string(),
        ),
        Line::from(""),
        Line::from(format!("Eligible                   {}", summary.eligible)),
        Line::from(format!("Excluded                   {}", summary.excluded)),
        Line::from(format!(
            "Problem PLUs               {}",
            summary.problem_count
        )),
    ]
}

fn reference_text_from_analysis(app: &TerminalApp, width: u16) -> String {
    app.analysis
        .as_ref()
        .map(|summary| {
            compact_references(
                &summary.departments,
                &summary.groups,
                &summary.label_formats,
                width.saturating_sub(8),
            )
        })
        .unwrap_or_else(|| "Run analysis first.".to_string())
}

fn render_readiness_body(app: &TerminalApp) -> Vec<Line<'static>> {
    let Some(summary) = &app.readiness else {
        return vec![Line::from("Readiness has not run yet.")];
    };
    let mut lines = vec![
        status_line(true, "Configuration", "Checked"),
        status_line(true, "MDB", CANONICAL_MDB_DISPLAY_PATH),
        status_line(
            summary.unverified_references == 0,
            "References confirmed",
            if summary.unverified_references == 0 {
                "OK"
            } else {
                "Incomplete"
            },
        ),
        status_line(
            summary.authentication == AuthenticationReadinessStatus::Passed,
            "Authentication",
            summary.authentication.as_str(),
        ),
        Line::from(""),
        Line::from(summary.readiness.as_str().to_string()),
        Line::from(format!("Eligible {}", summary.eligible)),
        Line::from(format!("Excluded {}", summary.excluded)),
    ];
    if summary.readiness == ReadinessResult::NotReady {
        lines.push(Line::from("Live import is blocked."));
    }
    lines
}

fn render_dry_run_body(app: &TerminalApp) -> Vec<Line<'static>> {
    let Some(summary) = &app.dry_run else {
        return vec![Line::from(
            "First available uses --test. Specific PLU uses exact --plu. All eligible submits zero API writes.",
        )];
    };
    let mut lines = Vec::new();
    if let Some(plu) = summary.selected_plu {
        lines.push(Line::from(format!("PLU {plu}")));
        lines.push(Line::from(""));
    }
    lines.push(status_line(
        summary.success,
        "Payload valid",
        if summary.success { "OK" } else { "Review" },
    ));
    lines.push(status_line(
        summary.api_write_requests == 0,
        "No API writes",
        "0",
    ));
    lines.push(Line::from(""));
    lines.push(Line::from(format!("Selected {}", summary.selected)));
    lines.push(Line::from(format!("Would submit {}", summary.would_submit)));
    lines.push(Line::from(format!("Eligible {}", summary.eligible)));
    lines.push(Line::from(format!("Excluded {}", summary.excluded)));
    lines
}

fn render_full_import_confirm_body(app: &TerminalApp) -> Vec<Line<'static>> {
    let config = AppConfig::load(Path::new("config.toml")).unwrap_or_default();
    let is_single = app.live_selection.is_some();
    let mut lines = vec![
        Line::from(if is_single {
            "Live Import"
        } else {
            "Full Import"
        }),
        Line::from(""),
        Line::from(format!(
            "Server:      {}",
            host_from_base_url(&config.digiweb.base_url)
        )),
        Line::from(format!(
            "Profile:     {}",
            app.selected_profile.as_deref().unwrap_or("Generic")
        )),
    ];
    if let Some(analysis) = &app.analysis {
        lines.push(Line::from(format!("Eligible:    {}", analysis.eligible)));
        lines.push(Line::from(format!("Excluded:    {}", analysis.excluded)));
    }
    if let Some(selection) = app.live_selection {
        if let Some(plu) = selection.requested_plu {
            lines.push(Line::from(format!("PLU:         {plu}")));
        } else if selection.test_mode {
            lines.push(Line::from("PLU:         First available"));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from("This will modify DIGIweb."));
    lines.push(Line::from("Cancel is selected by default."));
    lines
}

pub(super) fn render_full_import_progress_body(app: &TerminalApp) -> Vec<Line<'static>> {
    let Some(snapshot) = &app.import_progress else {
        return vec![
            Line::from("Full Import"),
            Line::from(""),
            Line::from("Starting importer..."),
        ];
    };
    vec![
        Line::from("Full Import"),
        Line::from(""),
        Line::from(format!(
            "{}/{} ({:.1}%)",
            snapshot.completed,
            snapshot.selected,
            snapshot.percent()
        )),
        Line::from(""),
        status_line(true, "Successful", &snapshot.success.to_string()),
        status_line(snapshot.failed == 0, "Failed", &snapshot.failed.to_string()),
        status_line(
            snapshot.unknown == 0,
            "Unknown",
            &snapshot.unknown.to_string(),
        ),
        Line::from(format!("Active {}", snapshot.active)),
        Line::from(format!("Remaining {}", snapshot.remaining)),
        Line::from(format!("{:.1} PLUs/s", snapshot.rate_per_second())),
        Line::from(format!(
            "{} elapsed",
            format_progress_duration(snapshot.elapsed)
        )),
        Line::from(format!(
            "{} remaining",
            snapshot
                .eta()
                .map(format_progress_duration)
                .unwrap_or_else(|| "--:--".to_string())
        )),
    ]
}

fn render_completion_body(app: &TerminalApp) -> Vec<Line<'static>> {
    let Some(summary) = &app.import_result else {
        return vec![Line::from("No import result is available.")];
    };
    if app.last_live_import_was_single {
        let plu = summary
            .selected_plu
            .map(|plu| plu.to_string())
            .unwrap_or_else(|| "selected".to_string());
        return vec![
            Line::from(format!("PLU {plu}")),
            Line::from(""),
            status_line(
                summary.failed == 0 && summary.unknown == 0,
                if summary.failed == 0 && summary.unknown == 0 {
                    "Import successful"
                } else {
                    "Import needs review"
                },
                summary.status.as_str(),
            ),
            Line::from(format!("Submitted {}", summary.submitted)),
            Line::from(format!("Successful {}", summary.succeeded)),
            Line::from(format!("Failed {}", summary.failed)),
            Line::from(format!("Unknown {}", summary.unknown)),
        ];
    }
    let mut lines = vec![
        Line::from(if summary.failed == 0 && summary.unknown == 0 {
            "Import Complete"
        } else {
            "Import Completed With Issues"
        }),
        Line::from(""),
        status_line(true, "Successful", &summary.succeeded.to_string()),
        status_line(summary.failed == 0, "Failed", &summary.failed.to_string()),
        status_line(
            summary.unknown == 0,
            "Unknown",
            &summary.unknown.to_string(),
        ),
        Line::from(format!("Submitted {}", summary.submitted)),
        Line::from(format!("Selected {}", summary.selected)),
        Line::from(format!("Status {}", summary.status)),
    ];
    if let Some(plu) = summary.selected_plu {
        lines.push(Line::from(format!("PLU {plu}")));
    }
    lines
}

fn format_progress_duration(duration: Duration) -> String {
    let total = duration.as_secs();
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn body_area(area: Rect) -> Rect {
    Rect {
        x: area.x + 2,
        y: area.y + 3,
        width: area.width.saturating_sub(4),
        height: 1,
    }
}

fn render_menu(frame: &mut ratatui::Frame<'_>, area: Rect, app: &TerminalApp) {
    let items = app
        .visible_items()
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let prefix = if index == app.selected_index {
                "> "
            } else {
                "  "
            };
            let style = if index == app.selected_index {
                selected_style()
            } else {
                Style::default()
            };
            ListItem::new(Line::from(format!("{prefix}{item}"))).style(style)
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("Actions")),
        area,
    );
}

fn status_line(ok: bool, label: &str, value: &str) -> Line<'static> {
    let symbol = if ok { "OK" } else { "WARN" };
    Line::from(format!("{symbol} {label:<24} {value}"))
}

fn selected_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

fn secondary_style() -> Style {
    Style::default().fg(Color::Gray)
}

pub fn logo_lines(mode: LogoMode) -> Vec<String> {
    match mode {
        LogoMode::Full => vec![
            "============        DIGI".to_string(),
            " ==========     to-digi-rs".to_string(),
            "  ========      DIGIweb Import Tool".to_string(),
            "   ======".to_string(),
            "    ====".to_string(),
            "     ==        ".to_string(),
        ],
        LogoMode::Compact => vec!["v DIGI   to-digi-rs".to_string()],
    }
}

pub fn compact_references(
    departments: &[u32],
    groups: &[(u32, u32)],
    label_formats: &[u32],
    width: u16,
) -> String {
    let departments = sorted_unique(departments);
    let label_formats = sorted_unique(label_formats);
    let mut group_map: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for (department, group) in groups {
        group_map.entry(*department).or_default().insert(*group);
    }

    let mut lines = Vec::new();
    lines.push(format!("Departments: {}", join_u32s(&departments)));
    lines.push("Groups:".to_string());
    for (department, values) in group_map {
        let values = values.into_iter().collect::<Vec<_>>();
        let prefix = format!("  Department {department:02}: ");
        push_wrapped_line(&mut lines, &prefix, &join_u32s(&values), width);
    }
    lines.push(format!("Label formats: {}", join_u32s(&label_formats)));
    lines.join("\n")
}

fn sorted_unique(values: &[u32]) -> Vec<u32> {
    values
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn join_u32s(values: &[u32]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn push_wrapped_line(lines: &mut Vec<String>, prefix: &str, value: &str, width: u16) {
    let width = usize::from(width.max(32));
    if prefix.len() + value.len() <= width {
        lines.push(format!("{prefix}{value}"));
        return;
    }

    let indent = " ".repeat(prefix.len());
    let mut current = prefix.to_string();
    for part in value.split(", ") {
        let separator = if current.trim_end().ends_with(':') {
            ""
        } else {
            ", "
        };
        if current.len() + separator.len() + part.len() > width && current != prefix {
            lines.push(current);
            current = format!("{indent}{part}");
        } else {
            current.push_str(separator);
            current.push_str(part);
        }
    }
    lines.push(current);
}
