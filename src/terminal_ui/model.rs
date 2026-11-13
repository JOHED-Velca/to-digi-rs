use crate::digiweb::preflight::{AuthenticationReadinessStatus, ReadinessResult};
use crate::selection::SelectionCriteria;
pub const CANONICAL_MDB_DISPLAY_PATH: &str = "~/digi/plu.mdb";

#[derive(Debug, Clone)]
pub struct TerminalUiOptions {
    pub default_profile: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Main,
    DeploymentCheck,
    DigiwebConfig,
    Profile,
    Analysis,
    Diagnostics,
    References,
    ReadinessReady,
    DryRun,
    SinglePlu,
    FullImportErrorHandling,
    FullImportConfirm,
    FullImportProgress,
    Completion,
    Error,
    Version,
    Details,
}

impl Screen {
    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Main => "to-digi-rs",
            Self::DeploymentCheck => "Deployment Check",
            Self::DigiwebConfig => "DIGIweb Configuration",
            Self::Profile => "Customer Profile",
            Self::Analysis => "MDB Analysis",
            Self::Diagnostics => "Diagnostics",
            Self::References => "Required DIGIweb References",
            Self::ReadinessReady => "Readiness",
            Self::DryRun => "Dry Run",
            Self::SinglePlu => "Single PLU Test",
            Self::FullImportErrorHandling => "Full Import Error Handling",
            Self::FullImportConfirm => "Full Import",
            Self::FullImportProgress => "Full Import",
            Self::Completion => "Import Complete",
            Self::Error => "Error",
            Self::Version => "Version / About",
            Self::Details => "Details",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogoMode {
    Full,
    Compact,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiOperation {
    DeploymentCheck,
    SaveHost(String),
    SaveSecret(String),
    Analyze,
    Diagnose,
    ConfirmReferences,
    Verify,
    DryRun(SelectionCriteria),
    Import {
        selection: SelectionCriteria,
        continue_on_error: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    DigiwebHost,
    Secret,
    DryRunPlu,
    LivePlu,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeploymentSummary {
    pub mdb_found: bool,
    pub config_ready: bool,
    pub profiles_ready: bool,
    pub config_error: Option<String>,
}

impl DeploymentSummary {
    pub(super) fn ready(&self) -> bool {
        self.mdb_found && self.config_ready && self.profiles_ready
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnalysisSummary {
    pub source_plus: usize,
    pub eligible: usize,
    pub excluded: usize,
    pub departments: Vec<u32>,
    pub groups: Vec<(u32, u32)>,
    pub label_formats: Vec<u32>,
    pub warning_count: usize,
    pub blocking_count: usize,
    pub warning_messages: Vec<String>,
    pub blocking_messages: Vec<String>,
    pub artifact_paths: Vec<String>,
    pub first_valid_plu: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagnosticsSummaryView {
    pub eligible: usize,
    pub excluded: usize,
    pub missing_name_count: usize,
    pub duplicate_barcode_count: usize,
    pub problem_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadinessSummaryView {
    pub readiness: ReadinessResult,
    pub authentication: AuthenticationReadinessStatus,
    pub authentication_error: Option<String>,
    pub eligible: usize,
    pub excluded: usize,
    pub unverified_references: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DryRunSummaryView {
    pub selected: usize,
    pub would_submit: usize,
    pub api_write_requests: usize,
    pub selected_plu: Option<u64>,
    pub eligible: usize,
    pub excluded: usize,
    pub success: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportSummaryView {
    pub selected: usize,
    pub submitted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub unknown: usize,
    pub not_attempted: usize,
    pub selected_plu: Option<u64>,
    pub continue_on_error: bool,
    pub failures: Vec<ImportFailureView>,
    pub needs_manual_check: Vec<u64>,
    pub status: String,
    pub artifact_paths: Vec<String>,
    pub single_success_eligible: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportFailureView {
    pub plu_number: u64,
    pub status: String,
    pub stage: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DetailsContext {
    #[default]
    None,
    Analysis,
    Readiness,
    Diagnostics,
    DryRun,
    SingleImport,
    FullImport,
}
