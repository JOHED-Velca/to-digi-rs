pub mod report;

pub use report::{
    VerificationDiscoveryInput, build_discovery_blocked_report, render_console_summary,
    write_json_report, write_text_report,
};
