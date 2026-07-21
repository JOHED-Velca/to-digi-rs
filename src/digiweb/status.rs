use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessingStatus {
    Success,
    Fail,
    Processing,
    SubmittedStatusUnknown,
    UnknownOrTimeout,
}

impl ProcessingStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Fail => "FAIL",
            Self::Processing => "PROCESSING",
            Self::SubmittedStatusUnknown => "SUBMITTED_STATUS_UNKNOWN",
            Self::UnknownOrTimeout => "UNKNOWN_OR_TIMEOUT",
        }
    }
}

impl Serialize for ProcessingStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ProcessingStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.to_ascii_uppercase().as_str() {
            "SUCCESS" | "SUCCEEDED" | "OK" => Self::Success,
            "FAIL" | "FAILED" | "ERROR" => Self::Fail,
            "PROCESSING" | "PENDING" | "RUNNING" => Self::Processing,
            "SUBMITTED_STATUS_UNKNOWN" => Self::SubmittedStatusUnknown,
            "UNKNOWN_OR_TIMEOUT" => Self::UnknownOrTimeout,
            _ => Self::Fail,
        })
    }
}
