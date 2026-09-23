use crate::cli::{
    AnalyzeArgs, Cli, CliCommand, ConfirmArgs, ConfirmTarget, DiagnoseArgs, DryRunArgs, ImportArgs,
    VerifyArgs,
};
use crate::digiweb::preflight::{AuthenticationReadinessStatus, ReadinessResult};
use crate::selection::SelectionCriteria;
pub const CANONICAL_MDB_DISPLAY_PATH: &str = "~/digi/plu.mdb";

#[derive(Debug, Clone)]
pub enum TerminalExit {
    Quit,
    Command(Cli),
}

#[derive(Debug, Clone)]
pub struct TerminalUiOptions {
    pub default_profile: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
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
    FullImportConfirm,
    FullImportProgress,
    Completion,
    Resume,
    Advanced,
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
            Self::FullImportConfirm => "Full Import",
            Self::FullImportProgress => "Full Import",
            Self::Completion => "Import Complete",
            Self::Resume => "Previous Imports",
            Self::Advanced => "Advanced Tools",
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
#[allow(dead_code)]
pub enum UiAction {
    Analyze { profile: Option<String> },
    DiagnoseInvalidOnly { profile: Option<String> },
    ConfirmAll { profile: Option<String> },
    Verify { profile: Option<String> },
    DryRunFirst { profile: Option<String> },
    DryRunSpecific { profile: Option<String>, plu: u64 },
    ImportFirst { profile: Option<String> },
    ImportSpecific { profile: Option<String>, plu: u64 },
    ImportAll { profile: Option<String> },
    ConfigHost(String),
    ConfigSecret,
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
    Import(SelectionCriteria),
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
    pub selected_plu: Option<u64>,
    pub status: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(dead_code)]
pub enum DetailsContext {
    #[default]
    None,
    Readiness,
    Diagnostics,
    DryRun,
    SingleImport,
    FullImport,
    Configuration,
}

impl UiAction {
    pub fn into_cli(self) -> Cli {
        let command = match self {
            Self::Analyze { profile } => CliCommand::Analyze(AnalyzeArgs {
                sanitize_profile: None,
                profile,
                raw: false,
            }),
            Self::DiagnoseInvalidOnly { profile } => CliCommand::Diagnose(DiagnoseArgs {
                invalid_only: true,
                plu: None,
                category: None,
                sanitize_profile: None,
                profile,
            }),
            Self::ConfirmAll { profile } => CliCommand::Confirm(ConfirmArgs {
                target: ConfirmTarget::All,
                sanitize_profile: None,
                profile,
                dry_run: false,
                yes: false,
            }),
            Self::Verify { profile } => CliCommand::Verify(VerifyArgs {
                sanitize_profile: None,
                profile,
            }),
            Self::DryRunFirst { profile } => CliCommand::DryRun(DryRunArgs {
                limit: None,
                test: true,
                plu: None,
                sanitize_profile: None,
                profile,
            }),
            Self::DryRunSpecific { profile, plu } => CliCommand::DryRun(DryRunArgs {
                limit: None,
                test: false,
                plu: Some(plu),
                sanitize_profile: None,
                profile,
            }),
            Self::ImportFirst { profile } => CliCommand::Import(import_args(profile, true, None)),
            Self::ImportSpecific { profile, plu } => {
                CliCommand::Import(import_args(profile, false, Some(plu)))
            }
            Self::ImportAll { profile } => CliCommand::Import(import_args(profile, false, None)),
            Self::ConfigHost(host) => {
                let mut args = import_args(None, true, None);
                args.config_ip = Some(host);
                CliCommand::Import(args)
            }
            Self::ConfigSecret => {
                let mut args = import_args(None, true, None);
                args.config_secret = true;
                CliCommand::Import(args)
            }
        };
        Cli {
            command: Some(command),
        }
    }
}

fn import_args(profile: Option<String>, test: bool, plu: Option<u64>) -> ImportArgs {
    ImportArgs {
        limit: None,
        test,
        plu,
        dry_run: false,
        continue_on_error: false,
        resume: None,
        retry_failed: false,
        sanitize_profile: None,
        profile,
        config_ip: None,
        config_secret: false,
        config_secret_stdin: false,
    }
}
