use crate::selection::SelectionCriteria;

pub(super) fn single_import_success_eligible(
    selection: SelectionCriteria,
    selected: usize,
    succeeded: usize,
    failed: usize,
    unknown: usize,
    not_attempted: usize,
) -> bool {
    (selection.test_mode || selection.requested_plu.is_some())
        && selected == 1
        && succeeded == 1
        && failed == 0
        && unknown == 0
        && not_attempted == 0
}

pub(super) fn operation_status(
    primary_error: Option<String>,
    logging_error: Option<String>,
) -> String {
    match (primary_error, logging_error) {
        (Some(primary), Some(logging)) => format!("{primary} (logging also failed: {logging})"),
        (Some(primary), None) => primary,
        (None, Some(logging)) => format!("Logging error: {logging}"),
        (None, None) => "Operation complete.".to_string(),
    }
}
