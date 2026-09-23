use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::digiweb::preflight::ReadinessResult;
use crate::selection::SelectionCriteria;

use super::actions::current_digiweb_host_text;
use super::app::TerminalApp;
use super::model::{DeploymentSummary, DetailsContext, InputMode, Screen, UiOperation};

impl TerminalApp {
    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.input_mode.is_some() {
            self.handle_input_key(key);
            return;
        }
        if self.busy_message.is_some() {
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => {}
            KeyCode::Esc => self.go_back(),
            KeyCode::Up if self.screen == Screen::Details => {
                self.details_scroll = self.details_scroll.saturating_sub(1)
            }
            KeyCode::Down if self.screen == Screen::Details => {
                self.details_scroll = self.details_scroll.saturating_add(1)
            }
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::Enter => self.select_current(),
            _ => {}
        }
    }

    pub(super) fn move_selection(&mut self, delta: isize) {
        let len = self.visible_items().len();
        if len == 0 {
            return;
        }
        self.selected_index = if delta < 0 {
            self.selected_index.checked_sub(1).unwrap_or(len - 1)
        } else {
            (self.selected_index + 1) % len
        };
    }

    pub(super) fn go_back(&mut self) {
        if let Some(screen) = self.history.pop() {
            self.screen = screen;
            self.selected_index = 0;
        } else if self.screen == Screen::Main {
            self.selected_index = self
                .selected_index
                .min(self.visible_items().len().saturating_sub(1));
        } else {
            self.screen = Screen::Main;
            self.selected_index = 0;
        }
    }

    pub(super) fn push_screen(&mut self, screen: Screen) {
        self.history.push(self.screen);
        self.screen = screen;
        self.selected_index = 0;
        if screen == Screen::Details {
            self.details_scroll = 0;
        }
        if matches!(screen, Screen::References | Screen::FullImportConfirm) {
            self.selected_index = 0;
        }
    }

    pub(super) fn push_details(&mut self, context: DetailsContext) {
        self.details_context = context;
        self.push_screen(Screen::Details);
    }

    pub(super) fn push_screen_with_operation(&mut self, screen: Screen, operation: UiOperation) {
        self.push_screen(screen);
        self.pending_operation = Some(operation);
    }

    fn select_current(&mut self) {
        match self.screen {
            Screen::Main => match self.selected_index {
                0 => self.push_screen_with_operation(
                    Screen::DeploymentCheck,
                    UiOperation::DeploymentCheck,
                ),
                1 => self.push_screen_with_operation(Screen::Analysis, UiOperation::Analyze),
                2 => self.push_screen_with_operation(Screen::Diagnostics, UiOperation::Diagnose),
                3 => self.push_screen(Screen::DigiwebConfig),
                4 => self.push_screen_with_operation(Screen::ReadinessReady, UiOperation::Verify),
                5 => self.push_screen(Screen::Advanced),
                6 => self.push_screen(Screen::Version),
                _ => self.should_quit = true,
            },
            Screen::DeploymentCheck => match self.selected_index {
                0 if self
                    .deployment
                    .as_ref()
                    .is_some_and(DeploymentSummary::ready) =>
                {
                    self.push_screen(Screen::DigiwebConfig)
                }
                0 => self.pending_operation = Some(UiOperation::DeploymentCheck),
                2 => self.should_quit = true,
                1 => self.go_back(),
                _ => {}
            },
            Screen::DigiwebConfig => match self.selected_index {
                0 => self.push_screen(Screen::Profile),
                1 => self.start_input(InputMode::DigiwebHost, current_digiweb_host_text()),
                2 => self.start_input(InputMode::Secret, ""),
                _ => self.go_back(),
            },
            Screen::Profile => match self.selected_index {
                0 => self.selected_profile = None,
                1 => self.selected_profile = Some("bigway".to_string()),
                2 => self.selected_profile = Some("starsky".to_string()),
                4 => self.push_screen_with_operation(Screen::Analysis, UiOperation::Analyze),
                _ => {}
            },
            Screen::Analysis => match self.selected_index {
                0 => self.push_screen_with_operation(Screen::Diagnostics, UiOperation::Diagnose),
                1 => self.push_details(DetailsContext::Diagnostics),
                _ => self.go_back(),
            },
            Screen::Diagnostics => match self.selected_index {
                0 => self.push_screen(Screen::References),
                1 => self.push_details(DetailsContext::Diagnostics),
                _ => self.go_back(),
            },
            Screen::References => match self.selected_index {
                1 => self.pending_operation = Some(UiOperation::ConfirmReferences),
                2 => self.go_back(),
                _ => {}
            },
            Screen::ReadinessReady => match self.selected_index {
                0 if self.readiness.is_none() => self.pending_operation = Some(UiOperation::Verify),
                0 if self
                    .readiness
                    .as_ref()
                    .is_some_and(|summary| summary.readiness != ReadinessResult::NotReady) =>
                {
                    self.dry_run = None;
                    self.push_screen(Screen::DryRun)
                }
                0 => self.pending_operation = Some(UiOperation::Verify),
                1 => self.push_details(DetailsContext::Readiness),
                _ => self.go_back(),
            },
            Screen::DryRun => {
                if self.dry_run.is_some() {
                    match self.selected_index {
                        0 => self.push_screen(Screen::SinglePlu),
                        1 => self.push_details(DetailsContext::DryRun),
                        _ => self.go_back(),
                    }
                } else {
                    match self.selected_index {
                        0 => {
                            self.pending_operation = Some(UiOperation::DryRun(SelectionCriteria {
                                limit: Some(1),
                                requested_plu: None,
                                test_mode: true,
                            }))
                        }
                        1 => self.start_input(InputMode::DryRunPlu, ""),
                        2 => {
                            self.pending_operation = Some(UiOperation::DryRun(SelectionCriteria {
                                limit: None,
                                requested_plu: None,
                                test_mode: false,
                            }))
                        }
                        3 => self.push_screen(Screen::SinglePlu),
                        _ => self.go_back(),
                    }
                }
            }
            Screen::SinglePlu => match self.selected_index {
                0 => {
                    self.live_selection = Some(SelectionCriteria {
                        limit: Some(1),
                        requested_plu: None,
                        test_mode: true,
                    });
                    self.push_screen(Screen::FullImportConfirm);
                }
                1 => self.start_input(InputMode::LivePlu, ""),
                _ => self.go_back(),
            },
            Screen::FullImportConfirm => match self.selected_index {
                1 => {
                    let selection = self.live_selection.unwrap_or(SelectionCriteria {
                        limit: None,
                        requested_plu: None,
                        test_mode: false,
                    });
                    self.last_live_import_was_single = self.live_selection.is_some();
                    self.pending_operation = Some(UiOperation::Import(selection));
                }
                _ => self.go_back(),
            },
            Screen::Completion => match self.selected_index {
                0 if self.last_live_import_was_single => {
                    self.live_selection = None;
                    self.last_live_import_was_single = false;
                    self.push_screen(Screen::FullImportConfirm);
                }
                0 => self.screen = Screen::Main,
                1 => self.push_details(if self.last_live_import_was_single {
                    DetailsContext::SingleImport
                } else {
                    DetailsContext::FullImport
                }),
                _ => self.should_quit = true,
            },
            Screen::Advanced => match self.selected_index {
                0 => self.push_screen(Screen::Version),
                _ => self.go_back(),
            },
            Screen::Version | Screen::Details | Screen::Resume => self.go_back(),
            Screen::FullImportProgress => {}
        }
    }

    fn handle_input_key(&mut self, key: KeyEvent) {
        let Some(mode) = self.input_mode else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.input_mode = None;
                self.input_buffer.clear();
                self.input_error = None;
            }
            KeyCode::Backspace => {
                self.input_buffer.pop();
            }
            KeyCode::Enter => match mode {
                InputMode::DigiwebHost => {
                    self.pending_operation = Some(UiOperation::SaveHost(self.input_buffer.clone()))
                }
                InputMode::Secret => {
                    let secret = std::mem::take(&mut self.input_buffer);
                    self.pending_operation = Some(UiOperation::SaveSecret(secret));
                }
                InputMode::DryRunPlu | InputMode::LivePlu => {
                    match parse_plu_input(&self.input_buffer) {
                        Ok(plu) => {
                            self.input_mode = None;
                            self.input_buffer.clear();
                            let criteria = SelectionCriteria {
                                limit: None,
                                requested_plu: Some(plu),
                                test_mode: false,
                            };
                            if mode == InputMode::DryRunPlu {
                                self.pending_operation = Some(UiOperation::DryRun(criteria));
                            } else {
                                self.live_selection = Some(criteria);
                                self.push_screen(Screen::FullImportConfirm);
                            }
                        }
                        Err(err) => self.input_error = Some(err),
                    }
                }
            },
            KeyCode::Char(ch) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    return;
                }
                match mode {
                    InputMode::DigiwebHost => self.input_buffer.push(ch),
                    InputMode::Secret => self.input_buffer.push(ch),
                    InputMode::DryRunPlu | InputMode::LivePlu if ch.is_ascii_digit() => {
                        self.input_buffer.push(ch)
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn parse_plu_input(value: &str) -> Result<u64, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("PLU number is required".to_string());
    }
    let plu = trimmed
        .parse::<u64>()
        .map_err(|_| "PLU number must be numeric".to_string())?;
    if plu == 0 {
        return Err("PLU number must be greater than zero".to_string());
    }
    Ok(plu)
}
