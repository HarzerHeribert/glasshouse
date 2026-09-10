use crossterm::event::{KeyCode, KeyEvent};

use crate::profile::LaunchProfile;
use crate::session::SessionPresentation;

use super::{Action, Overlay, ShellState};

/// The enabled profiles available for one already-selected harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileChoice {
    pub options: Vec<LaunchProfile>,
    pub cursor: usize,
    pub presentation: SessionPresentation,
}

impl ShellState {
    pub fn open_profile_choice(
        &mut self,
        options: Vec<LaunchProfile>,
        presentation: SessionPresentation,
    ) -> Action {
        if options.is_empty() {
            self.set_status("no enabled launch profile is available");
            return Action::Redraw;
        }
        self.profile_choice = Some(ProfileChoice {
            options,
            cursor: 0,
            presentation,
        });
        self.overlay = Some(Overlay::ProfileChoice);
        Action::Redraw
    }

    pub fn profile_choice(&self) -> Option<&ProfileChoice> {
        self.profile_choice.as_ref()
    }

    pub(super) fn handle_profile_choice_key(&mut self, key: KeyEvent) -> Action {
        let Some(choice) = self.profile_choice.as_mut() else {
            return self.close_overlay();
        };
        let len = choice.options.len();
        match key.code {
            KeyCode::Esc => {
                self.profile_choice = None;
                self.close_overlay()
            }
            KeyCode::Up | KeyCode::Char('k') => {
                choice.cursor = choice.cursor.checked_sub(1).unwrap_or(len - 1);
                Action::Redraw
            }
            KeyCode::Down | KeyCode::Char('j') => {
                choice.cursor = (choice.cursor + 1) % len;
                Action::Redraw
            }
            KeyCode::Enter => {
                let profile = choice.options[choice.cursor].clone();
                let presentation = choice.presentation;
                self.profile_choice = None;
                self.overlay = None;
                Action::StartSessionWithProfile {
                    harness: profile.harness,
                    profile: profile.name,
                    presentation,
                }
            }
            _ => Action::None,
        }
    }
}
