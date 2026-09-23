use std::fmt;

use crate::digiweb::preflight::ReadinessResult;
use crate::import::runner::ImportProgressSnapshot;
use crate::selection::SelectionCriteria;

use super::model::{
    AnalysisSummary, DeploymentSummary, DetailsContext, DiagnosticsSummaryView, DryRunSummaryView,
    ImportSummaryView, InputMode, ReadinessSummaryView, Screen, TerminalUiOptions, UiAction,
    UiOperation,
};
pub struct TerminalApp {
    pub screen: Screen,
    pub selected_index: usize,
    pub(super) history: Vec<Screen>,
    pub selected_profile: Option<String>,
    pub should_quit: bool,
    pub pending_action: Option<UiAction>,
    pub pending_operation: Option<UiOperation>,
    pub deployment: Option<DeploymentSummary>,
    pub analysis: Option<AnalysisSummary>,
    pub diagnostics: Option<DiagnosticsSummaryView>,
    pub readiness: Option<ReadinessSummaryView>,
    pub dry_run: Option<DryRunSummaryView>,
    pub import_result: Option<ImportSummaryView>,
    pub import_progress: Option<ImportProgressSnapshot>,
    pub details_context: DetailsContext,
    pub details_scroll: u16,
    pub input_mode: Option<InputMode>,
    pub input_buffer: String,
    pub input_error: Option<String>,
    pub status_message: Option<String>,
    pub busy_message: Option<String>,
    pub live_selection: Option<SelectionCriteria>,
    pub last_live_import_was_single: bool,
}
impl TerminalApp {
    pub fn new(options: TerminalUiOptions) -> Self {
        Self {
            screen: Screen::Main,
            selected_index: 0,
            history: Vec::new(),
            selected_profile: options
                .default_profile
                .filter(|value| !value.trim().is_empty()),
            should_quit: false,
            pending_action: None,
            pending_operation: None,
            deployment: None,
            analysis: None,
            diagnostics: None,
            readiness: None,
            dry_run: None,
            import_result: None,
            import_progress: None,
            details_context: DetailsContext::None,
            details_scroll: 0,
            input_mode: None,
            input_buffer: String::new(),
            input_error: None,
            status_message: None,
            busy_message: None,
            live_selection: None,
            last_live_import_was_single: false,
        }
    }

    pub fn start_input(&mut self, mode: InputMode, initial: impl Into<String>) {
        self.input_mode = Some(mode);
        self.input_buffer = initial.into();
        self.input_error = None;
    }

    pub fn visible_items(&self) -> Vec<&'static str> {
        if self.input_mode.is_some() {
            return vec!["Save", "Cancel"];
        }
        if self.busy_message.is_some() {
            return Vec::new();
        }
        match self.screen {
            Screen::Main => vec![
                "Guided Import",
                "Analyze MDB",
                "Diagnostics",
                "DIGIweb Configuration",
                "Readiness / Verification",
                "Advanced Tools",
                "Version / About",
                "Exit",
            ],
            Screen::DeploymentCheck => {
                if self
                    .deployment
                    .as_ref()
                    .is_some_and(DeploymentSummary::ready)
                {
                    vec!["Continue", "Back"]
                } else {
                    vec!["Check Again", "Back", "Exit"]
                }
            }
            Screen::DigiwebConfig => {
                vec![
                    "Continue",
                    "Change DIGIweb Host",
                    "Change Client Secret",
                    "Back",
                ]
            }
            Screen::Profile => vec![
                "Generic",
                "Bigway",
                "Starsky",
                "Custom...",
                "Continue",
                "Back",
            ],
            Screen::Analysis => vec!["Continue", "View Issues", "Back"],
            Screen::Diagnostics => vec!["Continue", "View Details", "Back"],
            Screen::References => vec!["No", "Yes, record confirmation", "Back"],
            Screen::ReadinessReady => match &self.readiness {
                None => vec!["Run Readiness Check", "Back"],
                Some(summary) if summary.readiness == ReadinessResult::NotReady => {
                    vec!["Run Readiness Check", "View Details", "Back"]
                }
                Some(_) => vec!["Continue", "View Details", "Back"],
            },
            Screen::DryRun => {
                if self.dry_run.is_some() {
                    vec!["Continue", "View Details", "Back"]
                } else {
                    vec![
                        "First available PLU",
                        "Specific PLU",
                        "All eligible PLUs",
                        "Skip",
                        "Back",
                    ]
                }
            }
            Screen::SinglePlu => vec!["First available PLU", "Specific PLU", "Back"],
            Screen::FullImportConfirm => {
                if self.live_selection.is_some() {
                    vec!["Cancel", "Import"]
                } else {
                    vec!["Cancel", "Start Import"]
                }
            }
            Screen::FullImportProgress => vec!["View Details"],
            Screen::Completion => {
                if self.last_live_import_was_single {
                    vec!["Continue", "View Details", "Back"]
                } else {
                    vec!["Main Menu", "View Details", "Exit"]
                }
            }
            Screen::Advanced => vec!["Version", "Back"],
            Screen::Resume => vec!["Back"],
            Screen::Version => vec!["Back"],
            Screen::Details => vec!["Back"],
        }
    }
}

impl fmt::Debug for TerminalApp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TerminalApp")
            .field("screen", &self.screen)
            .field("selected_index", &self.selected_index)
            .field("history", &self.history)
            .field("selected_profile", &self.selected_profile)
            .field("should_quit", &self.should_quit)
            .field("pending_action", &self.pending_action)
            .field("pending_operation", &self.pending_operation)
            .field("deployment", &self.deployment)
            .field("analysis", &self.analysis)
            .field("diagnostics", &self.diagnostics)
            .field("readiness", &self.readiness)
            .field("dry_run", &self.dry_run)
            .field("import_result", &self.import_result)
            .field("import_progress", &self.import_progress)
            .field("details_context", &self.details_context)
            .field("details_scroll", &self.details_scroll)
            .field("input_mode", &self.input_mode)
            .field(
                "input_buffer",
                &if self.input_mode == Some(InputMode::Secret) {
                    "<redacted>"
                } else {
                    self.input_buffer.as_str()
                },
            )
            .field("input_error", &self.input_error)
            .field("status_message", &self.status_message)
            .field("busy_message", &self.busy_message)
            .field("live_selection", &self.live_selection)
            .field(
                "last_live_import_was_single",
                &self.last_live_import_was_single,
            )
            .finish()
    }
}
