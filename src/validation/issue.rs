#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warning => "WARNING",
            Self::Info => "INFO",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub severity: Severity,
    pub plu_number: Option<u64>,
    pub field: String,
    pub message: String,
}

impl ValidationIssue {
    pub fn error(
        plu_number: Option<u64>,
        field: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity: Severity::Error,
            plu_number,
            field: field.into(),
            message: message.into(),
        }
    }

    pub fn warning(
        plu_number: Option<u64>,
        field: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity: Severity::Warning,
            plu_number,
            field: field.into(),
            message: message.into(),
        }
    }
}
