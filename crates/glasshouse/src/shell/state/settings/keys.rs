use super::*;

impl SettingsState {
    pub(super) fn new(
        harnesses: Vec<HarnessRow>,
        integrations: Vec<IntegrationRow>,
        providers: Vec<ProviderRow>,
        profiles: Vec<ProfileRow>,
        routing: RoutingRow,
        memory: MemoryRow,
    ) -> Self {
        Self {
            section: SettingsSection::Harnesses,
            harnesses,
            integrations,
            providers,
            profiles,
            routing,
            memory,
            selected_harness: 0,
            selected_integration: 0,
            selected_provider: 0,
            selected_profile: 0,
            edits: HashMap::new(),
            provider_edits: HashMap::new(),
            profile_edits: HashMap::new(),
            routing_edit: RoutingSettingsEdit::default(),
            memory_edit: MemorySettingsEdit::default(),
            path_input: None,
            confirm_project_write: false,
            confirm_credential_delete: None,
            provider_input: None,
            profile_input: None,
            routing_input: None,
            provider_notice: None,
            pending_probe: None,
            last_disposable_choice: None,
        }
    }

    pub fn section(&self) -> SettingsSection {
        self.section
    }

    pub fn harnesses(&self) -> &[HarnessRow] {
        &self.harnesses
    }

    pub fn integrations(&self) -> &[IntegrationRow] {
        &self.integrations
    }

    pub fn providers(&self) -> &[ProviderRow] {
        &self.providers
    }

    pub fn profiles(&self) -> &[ProfileRow] {
        &self.profiles
    }

    pub fn routing(&self) -> &RoutingRow {
        &self.routing
    }

    pub fn memory(&self) -> &MemoryRow {
        &self.memory
    }

    /// The most recent disposable-job routing choice, for the Routing
    /// section to render its reason from — see
    /// [`SettingsState::last_disposable_choice`]'s own field doc.
    pub fn last_disposable_choice(&self) -> Option<&DisposableChoice> {
        self.last_disposable_choice.as_ref()
    }

    pub(super) fn record_disposable_choice(&mut self, choice: DisposableChoice) {
        self.last_disposable_choice = Some(choice);
    }

    pub fn selected_harness(&self) -> usize {
        self.selected_harness
    }

    pub fn selected_integration(&self) -> usize {
        self.selected_integration
    }

    pub fn selected_provider(&self) -> usize {
        self.selected_provider
    }

    pub fn selected_profile(&self) -> usize {
        self.selected_profile
    }

    pub fn confirming_project_write(&self) -> bool {
        self.confirm_project_write
    }

    /// The provider whose stored credential is awaiting a `y`/Esc, if any.
    pub fn confirming_credential_delete(&self) -> Option<&str> {
        self.confirm_credential_delete.as_deref()
    }

    /// The active "add an explicit path" sub-mode, if any.
    pub fn path_input(&self) -> Option<SettingsPathInputView<'_>> {
        let input = self.path_input.as_ref()?;
        let harness_name = self.harnesses.get(self.selected_harness)?.id.display_name();
        Some(SettingsPathInputView {
            harness_name,
            buffer: input.buffer.as_str(),
            error: input.error.as_deref(),
        })
    }

    /// The active Providers-section text input, if any — a new provider's
    /// name then template, or an existing one's base URL or credential
    /// variable names.
    pub fn provider_input(&self) -> Option<ProviderInputView<'_>> {
        let input = self.provider_input.as_ref()?;
        let label = match &input.purpose {
            ProviderInputPurpose::NewName => "New provider name".to_owned(),
            ProviderInputPurpose::NewTemplate { name } => {
                format!("Template for `{name}` (openrouter, zai, openai-compatible, ...)")
            }
            ProviderInputPurpose::EditBaseUrl { name } => format!("Base URL for `{name}`"),
            ProviderInputPurpose::EditCredentialEnv { name } => {
                format!("Credential variable name(s) for `{name}`, comma-separated")
            }
            ProviderInputPurpose::EditFreeModels { name } => {
                format!("Free-tier model name(s) for `{name}`, comma-separated")
            }
            ProviderInputPurpose::SetCredential { name } => {
                format!("Credential for `{name}` (stored in the OS secure store, not shown)")
            }
        };
        // Masked here rather than in the renderer: the typed characters
        // never leave this method, so no view, snapshot or test harness can
        // reach them however it renders. The count of `*` follows the
        // buffer's length, which is what makes a field a user is typing into
        // usable at all — and is on that user's own screen, unlike a `Debug`
        // or a log line, where `ProviderTextInput` reveals nothing.
        let buffer = if input.purpose.is_secret() {
            "*".repeat(input.buffer.chars().count())
        } else {
            input.buffer.clone()
        };
        Some(ProviderInputView {
            label,
            buffer,
            error: input.error.as_deref(),
        })
    }

    /// The most recent connectivity check, if that is what the notice is —
    /// see [`ReachabilityCheck`].
    pub fn provider_test_result(&self) -> Option<(&str, &ReachabilityCheck)> {
        match self.provider_notice.as_ref()? {
            (name, ProviderNotice::Reachability(check)) => Some((name.as_str(), check)),
            (_, ProviderNotice::Models(_)) => None,
        }
    }

    /// The most recent model refresh, if that is what the notice is.
    pub fn provider_models_result(&self) -> Option<(&str, &ModelRefresh)> {
        match self.provider_notice.as_ref()? {
            (name, ProviderNotice::Models(refresh)) => Some((name.as_str(), refresh)),
            (_, ProviderNotice::Reachability(_)) => None,
        }
    }

    /// The active Launch-Profiles-section text input, if any — a new
    /// profile's name then harness, or an existing one's model or backend.
    pub fn profile_input(&self) -> Option<ProfileInputView<'_>> {
        let input = self.profile_input.as_ref()?;
        let label = match &input.purpose {
            ProfileInputPurpose::NewName => "New launch profile name".to_owned(),
            ProfileInputPurpose::NewHarness { name } => format!(
                "Harness for `{name}` ({})",
                known_launch_harnesses()
                    .map(|id| id.slug())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            ProfileInputPurpose::EditModel { name } => format!("Model override for `{name}`"),
            ProfileInputPurpose::EditBackend { name } => {
                format!("Backend for `{name}`: `native` or a configured provider name")
            }
            ProfileInputPurpose::Duplicate { source } => {
                format!("New name for a copy of `{source}`")
            }
        };
        Some(ProfileInputView {
            label,
            buffer: input.buffer.as_str(),
            error: input.error.as_deref(),
        })
    }

    /// The active Routing-section editor, if any.
    pub fn routing_input(&self) -> Option<RoutingInputView<'_>> {
        let input = self.routing_input.as_ref()?;
        let label = match input.purpose {
            RoutingInputPurpose::Model => {
                "Routing model (automatic, deterministic, or provider:model)"
            }
            RoutingInputPurpose::MaxLatency => "Maximum router latency (milliseconds)",
            RoutingInputPurpose::MaxCost => "Maximum marginal cost (USD per decision)",
            RoutingInputPurpose::PremiumReserve => "Premium reserve threshold (percent)",
            RoutingInputPurpose::FreeOrder => {
                "Free-resource order: provider:model, comma-separated"
            }
            RoutingInputPurpose::FreeDisabled => {
                "Disabled free resources: provider:model, comma-separated"
            }
            RoutingInputPurpose::FreePin => "Pinned free resource: provider:model, or empty",
        };
        Some(RoutingInputView {
            label,
            buffer: input.buffer.as_str(),
            error: input.error.as_deref(),
        })
    }

    /// Every pending harness edit, for the run loop to apply when saving.
    pub(super) fn edits(&self) -> Vec<SettingsEdit> {
        self.edits
            .iter()
            .map(|(&id, edit)| SettingsEdit {
                id,
                enabled: edit.enabled,
                executable: edit.executable.clone(),
            })
            .collect()
    }

    /// Every pending provider edit, for the run loop to apply when saving.
    pub(super) fn provider_edits(&self) -> Vec<ProviderSettingsEdit> {
        self.provider_edits
            .iter()
            .map(|(name, upsert)| ProviderSettingsEdit {
                name: name.clone(),
                upsert: upsert.clone(),
            })
            .collect()
    }

    /// Every pending profile edit, for the run loop to apply when saving.
    pub(super) fn profile_edits(&self) -> Vec<ProfileSettingsEdit> {
        self.profile_edits
            .iter()
            .map(|(name, upsert)| ProfileSettingsEdit {
                name: name.clone(),
                upsert: upsert.clone(),
            })
            .collect()
    }

    pub(super) fn routing_edit(&self) -> Option<RoutingSettingsEdit> {
        (!self.routing_edit.is_empty()).then(|| self.routing_edit.clone())
    }

    pub(super) fn memory_edit(&self) -> Option<MemorySettingsEdit> {
        (!self.memory_edit.is_empty()).then(|| self.memory_edit.clone())
    }

    /// Replace the rows with freshly loaded ones (after a successful save)
    /// and clear every pending edit. The catalog is fixed-size, so the
    /// cursor is only ever clamped, never reset, and always stays on a real
    /// row.
    pub(super) fn replace_rows(
        &mut self,
        harnesses: Vec<HarnessRow>,
        integrations: Vec<IntegrationRow>,
        providers: Vec<ProviderRow>,
        profiles: Vec<ProfileRow>,
        routing: RoutingRow,
        memory: MemoryRow,
    ) {
        self.selected_harness = self.selected_harness.min(harnesses.len().saturating_sub(1));
        self.selected_integration = self
            .selected_integration
            .min(integrations.len().saturating_sub(1));
        self.selected_provider = self
            .selected_provider
            .min(providers.len().saturating_sub(1));
        self.selected_profile = self.selected_profile.min(profiles.len().saturating_sub(1));
        self.harnesses = harnesses;
        self.integrations = integrations;
        self.providers = providers;
        self.profiles = profiles;
        self.routing = routing;
        self.memory = memory;
        self.edits.clear();
        self.provider_edits.clear();
        self.profile_edits.clear();
        self.routing_edit = RoutingSettingsEdit::default();
        self.memory_edit = MemorySettingsEdit::default();
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) -> SettingsAction {
        if self.path_input.is_some() {
            return self.handle_path_input_key(key);
        }
        if self.provider_input.is_some() {
            return self.handle_provider_input_key(key);
        }
        if self.profile_input.is_some() {
            return self.handle_profile_input_key(key);
        }
        if self.routing_input.is_some() {
            return self.handle_routing_input_key(key);
        }
        if let Some(provider) = self.confirm_credential_delete.clone() {
            return match key.code {
                KeyCode::Enter | KeyCode::Char('y') => {
                    self.confirm_credential_delete = None;
                    // The row must still be the one that was confirmed: a
                    // confirmation that deleted whatever happens to be
                    // selected now would be a different action from the one
                    // the user agreed to.
                    if self
                        .providers
                        .get(self.selected_provider)
                        .is_some_and(|row| row.name == provider)
                    {
                        SettingsAction::DeleteCredential
                    } else {
                        SettingsAction::Redraw
                    }
                }
                KeyCode::Esc | KeyCode::Char('n') => {
                    self.confirm_credential_delete = None;
                    SettingsAction::Redraw
                }
                // Swallowed, exactly like the project-write confirmation:
                // an explicit y/Enter or Esc/n, never "any key dismisses".
                _ => SettingsAction::None,
            };
        }

        if self.confirm_project_write {
            return match key.code {
                KeyCode::Enter | KeyCode::Char('y') => {
                    self.confirm_project_write = false;
                    SettingsAction::SaveProject
                }
                KeyCode::Esc | KeyCode::Char('n') => {
                    self.confirm_project_write = false;
                    SettingsAction::Redraw
                }
                // Anything else is swallowed: the design decision requires
                // an explicit y/Enter or Esc/n, not "any key dismisses".
                _ => SettingsAction::None,
            };
        }

        // A stale provider banner must not permanently shadow another
        // bottom-panel view — the wizard/field editors this session opens
        // afterward all render in that same area — so any key that reaches
        // this general dispatcher clears it first, exactly like the outer
        // shell's own status note clears on the next keystroke. The `t` and
        // `m` arms below set it again in the same keypress when that is what
        // was actually pressed.
        //
        // This clears a *banner*, never a request. An in-flight probe is on
        // `ProviderRow::activity` precisely so that pressing an arrow key
        // cannot make a running request invisible.
        self.provider_notice = None;

        match key.code {
            KeyCode::Esc => SettingsAction::Close,
            KeyCode::Char('w') => SettingsAction::SaveUser,
            KeyCode::Char('W') => {
                self.confirm_project_write = true;
                SettingsAction::Redraw
            }
            KeyCode::Char('r') => SettingsAction::ReopenOnboarding,
            KeyCode::Tab | KeyCode::Right => {
                self.section = self.section.next();
                SettingsAction::Redraw
            }
            KeyCode::BackTab | KeyCode::Left => {
                self.section = self.section.previous();
                SettingsAction::Redraw
            }
            KeyCode::Up => {
                self.move_selection(-1);
                SettingsAction::Redraw
            }
            KeyCode::Down => {
                self.move_selection(1);
                SettingsAction::Redraw
            }
            KeyCode::Char(' ') if self.section == SettingsSection::Harnesses => {
                self.toggle_selected_harness();
                SettingsAction::Redraw
            }
            KeyCode::Enter if self.section == SettingsSection::Harnesses => {
                if self.harnesses.get(self.selected_harness).is_some() {
                    self.path_input = Some(SettingsPathInput::default());
                }
                SettingsAction::Redraw
            }
            KeyCode::Char('a') if self.section == SettingsSection::Providers => {
                self.provider_input = Some(ProviderTextInput {
                    purpose: ProviderInputPurpose::NewName,
                    buffer: String::new(),
                    error: None,
                });
                SettingsAction::Redraw
            }
            KeyCode::Char('e') if self.section == SettingsSection::Providers => {
                self.start_edit_provider_base_url();
                SettingsAction::Redraw
            }
            KeyCode::Char('c') if self.section == SettingsSection::Providers => {
                self.start_edit_provider_credential_env();
                SettingsAction::Redraw
            }
            KeyCode::Char('f') if self.section == SettingsSection::Providers => {
                self.start_edit_provider_free_models();
                SettingsAction::Redraw
            }
            KeyCode::Char(' ') if self.section == SettingsSection::Providers => {
                self.toggle_selected_provider();
                SettingsAction::Redraw
            }
            KeyCode::Char('d') if self.section == SettingsSection::Providers => {
                self.remove_selected_provider();
                SettingsAction::Redraw
            }
            KeyCode::Char('t') if self.section == SettingsSection::Providers => {
                if self.begin_provider_test() {
                    SettingsAction::RunProviderProbe
                } else {
                    SettingsAction::Redraw
                }
            }
            KeyCode::Char('m') if self.section == SettingsSection::Providers => {
                if self.begin_provider_model_refresh() {
                    SettingsAction::RunProviderProbe
                } else {
                    SettingsAction::Redraw
                }
            }
            KeyCode::Char('s') if self.section == SettingsSection::Providers => {
                self.start_set_provider_credential();
                SettingsAction::Redraw
            }
            KeyCode::Char('x') if self.section == SettingsSection::Providers => {
                self.confirm_credential_delete = self
                    .providers
                    .get(self.selected_provider)
                    .map(|row| row.name.clone());
                SettingsAction::Redraw
            }
            KeyCode::Char('a') if self.section == SettingsSection::LaunchProfiles => {
                self.profile_input = Some(ProfileTextInput {
                    purpose: ProfileInputPurpose::NewName,
                    buffer: String::new(),
                    error: None,
                });
                SettingsAction::Redraw
            }
            KeyCode::Char('e') if self.section == SettingsSection::LaunchProfiles => {
                self.start_edit_profile_model();
                SettingsAction::Redraw
            }
            KeyCode::Char('b') if self.section == SettingsSection::LaunchProfiles => {
                self.start_edit_profile_backend();
                SettingsAction::Redraw
            }
            KeyCode::Char('p') if self.section == SettingsSection::LaunchProfiles => {
                self.cycle_selected_profile_approval();
                SettingsAction::Redraw
            }
            KeyCode::Char('u') if self.section == SettingsSection::LaunchProfiles => {
                self.start_duplicate_profile();
                SettingsAction::Redraw
            }
            KeyCode::Char(' ') if self.section == SettingsSection::LaunchProfiles => {
                self.toggle_selected_profile();
                SettingsAction::Redraw
            }
            KeyCode::Char('d') if self.section == SettingsSection::LaunchProfiles => {
                self.remove_selected_profile();
                SettingsAction::Redraw
            }
            KeyCode::Char('m') if self.section == SettingsSection::Routing => {
                self.start_routing_input(RoutingInputPurpose::Model);
                SettingsAction::Redraw
            }
            KeyCode::Char('l') if self.section == SettingsSection::Routing => {
                self.start_routing_input(RoutingInputPurpose::MaxLatency);
                SettingsAction::Redraw
            }
            KeyCode::Char('c') if self.section == SettingsSection::Routing => {
                self.start_routing_input(RoutingInputPurpose::MaxCost);
                SettingsAction::Redraw
            }
            KeyCode::Char('f') if self.section == SettingsSection::Routing => {
                self.routing.prefer_free = !self.routing.prefer_free;
                self.routing.prefer_free_layer = Layer::User;
                self.routing_edit.prefer_free = Some(self.routing.prefer_free);
                SettingsAction::Redraw
            }
            KeyCode::Char('p') if self.section == SettingsSection::Routing => {
                self.start_routing_input(RoutingInputPurpose::PremiumReserve);
                SettingsAction::Redraw
            }
            KeyCode::Char('o') if self.section == SettingsSection::Routing => {
                self.start_routing_input(RoutingInputPurpose::FreeOrder);
                SettingsAction::Redraw
            }
            KeyCode::Char('d') if self.section == SettingsSection::Routing => {
                self.start_routing_input(RoutingInputPurpose::FreeDisabled);
                SettingsAction::Redraw
            }
            KeyCode::Char('n') if self.section == SettingsSection::Routing => {
                self.start_routing_input(RoutingInputPurpose::FreePin);
                SettingsAction::Redraw
            }
            KeyCode::Char(' ') if self.section == SettingsSection::Memory => {
                self.memory.memory_extraction = !self.memory.memory_extraction;
                self.memory.memory_extraction_layer = Layer::User;
                self.memory_edit.memory_extraction = Some(self.memory.memory_extraction);
                SettingsAction::Redraw
            }
            _ => SettingsAction::None,
        }
    }

    fn move_selection(&mut self, delta: i32) {
        match self.section {
            SettingsSection::Harnesses => {
                if self.harnesses.is_empty() {
                    return;
                }
                let last = self.harnesses.len() as i32 - 1;
                self.selected_harness =
                    (self.selected_harness as i32 + delta).clamp(0, last) as usize;
            }
            SettingsSection::Integrations => {
                if self.integrations.is_empty() {
                    return;
                }
                let last = self.integrations.len() as i32 - 1;
                self.selected_integration =
                    (self.selected_integration as i32 + delta).clamp(0, last) as usize;
            }
            SettingsSection::Providers => {
                if self.providers.is_empty() {
                    return;
                }
                let last = self.providers.len() as i32 - 1;
                self.selected_provider =
                    (self.selected_provider as i32 + delta).clamp(0, last) as usize;
            }
            SettingsSection::LaunchProfiles => {
                if self.profiles.is_empty() {
                    return;
                }
                let last = self.profiles.len() as i32 - 1;
                self.selected_profile =
                    (self.selected_profile as i32 + delta).clamp(0, last) as usize;
            }
            SettingsSection::Routing | SettingsSection::Memory => {}
        }
    }

    fn toggle_selected_harness(&mut self) {
        let Some(row) = self.harnesses.get_mut(self.selected_harness) else {
            return;
        };
        row.enabled = !row.enabled;
        row.enabled_layer = Layer::User;
        self.edits.entry(row.id).or_default().enabled = Some(row.enabled);
    }

    // -------------------------------------------------------------
    // Providers
    // -------------------------------------------------------------

    fn start_edit_provider_base_url(&mut self) {
        let Some(row) = self.providers.get(self.selected_provider) else {
            return;
        };
        self.provider_input = Some(ProviderTextInput {
            purpose: ProviderInputPurpose::EditBaseUrl {
                name: row.name.clone(),
            },
            buffer: row.config.base_url().unwrap_or_default().to_owned(),
            error: None,
        });
    }

    fn start_edit_provider_credential_env(&mut self) {
        let Some(row) = self.providers.get(self.selected_provider) else {
            return;
        };
        self.provider_input = Some(ProviderTextInput {
            purpose: ProviderInputPurpose::EditCredentialEnv {
                name: row.name.clone(),
            },
            buffer: row.config.credential_env().join(","),
            error: None,
        });
    }

    fn start_edit_provider_free_models(&mut self) {
        let Some(row) = self.providers.get(self.selected_provider) else {
            return;
        };
        self.provider_input = Some(ProviderTextInput {
            purpose: ProviderInputPurpose::EditFreeModels {
                name: row.name.clone(),
            },
            buffer: row.config.free_models().join(","),
            error: None,
        });
    }

    /// Open the masked credential field for the selected provider.
    ///
    /// The buffer starts empty and is never pre-filled from anywhere: there
    /// is nothing to pre-fill it *with* that would not mean reading a
    /// credential out of a store in order to display it.
    fn start_set_provider_credential(&mut self) {
        let Some(row) = self.providers.get(self.selected_provider) else {
            return;
        };
        if row.config.credential_env().is_empty() {
            self.provider_input = Some(ProviderTextInput {
                purpose: ProviderInputPurpose::SetCredential {
                    name: row.name.clone(),
                },
                buffer: String::new(),
                error: Some(
                    "this provider names no credential variable yet — set one with `c` first, \
                     so the stored credential has a name to be found by"
                        .to_owned(),
                ),
            });
            return;
        }
        self.provider_input = Some(ProviderTextInput {
            purpose: ProviderInputPurpose::SetCredential {
                name: row.name.clone(),
            },
            buffer: String::new(),
            error: None,
        });
    }

    /// The provider the credential field is for, and what the user typed —
    /// **taken**, so this method can be called once and the value is gone
    /// from the overlay afterwards.
    pub(super) fn take_credential_entry(&mut self) -> Option<(String, String)> {
        let input = self.provider_input.take()?;
        let ProviderInputPurpose::SetCredential { name } = input.purpose else {
            // Not a credential field: put it back rather than discarding an
            // edit the user is in the middle of making.
            self.provider_input = Some(input);
            return None;
        };
        Some((name, input.buffer))
    }

    /// The selected provider and every reference under which its credential
    /// could be stored.
    ///
    /// More than one, because a provider may declare a pool of credential
    /// variable names and a stored reference may also have been recorded in
    /// configuration. Deleting means deleting all of them: a "delete my
    /// stored key" that left one of two copies behind would be a worse
    /// answer than raising.
    pub(super) fn selected_provider_stored_credentials(&self) -> Option<(String, Vec<SecretRef>)> {
        let row = self.providers.get(self.selected_provider)?;
        let mut references: Vec<SecretRef> = Vec::new();
        if let Some(stored) = row.config.credential_store() {
            references.push(stored.to_secret_ref());
        }
        for var in row.config.credential_env() {
            let reference = os_credential_for_variable(var);
            if !references.contains(&reference) {
                references.push(reference);
            }
        }
        Some((row.name.clone(), references))
    }

    /// Record that `provider`'s credential now lives in the OS store, and
    /// stage the configuration change that says so.
    pub(super) fn record_credential_stored(&mut self, provider: &str, stored: StoredCredentialRef) {
        let Some(row) = self.providers.iter_mut().find(|row| row.name == provider) else {
            return;
        };
        row.config.set_credential_store(Some(stored));
        row.layer = Layer::User;
        self.provider_edits
            .insert(row.name.clone(), Some(row.config.clone()));
    }

    /// The configuration half of deleting a stored credential: the reference
    /// goes, every other field stays.
    pub(super) fn record_credential_cleared(&mut self, provider: &str) {
        let Some(row) = self.providers.iter_mut().find(|row| row.name == provider) else {
            return;
        };
        if row.config.credential_store().is_none() {
            return;
        }
        row.config.set_credential_store(None);
        row.layer = Layer::User;
        self.provider_edits
            .insert(row.name.clone(), Some(row.config.clone()));
    }

    fn toggle_selected_provider(&mut self) {
        let Some(row) = self.providers.get_mut(self.selected_provider) else {
            return;
        };
        row.config.set_enabled(!row.config.enabled());
        row.layer = Layer::User;
        self.provider_edits
            .insert(row.name.clone(), Some(row.config.clone()));
    }

    fn remove_selected_provider(&mut self) {
        if self.selected_provider >= self.providers.len() {
            return;
        }
        let row = self.providers.remove(self.selected_provider);
        self.provider_edits.insert(row.name, None);
        self.selected_provider = self
            .selected_provider
            .min(self.providers.len().saturating_sub(1));
        self.provider_notice = None;
    }

    /// `t`: plan a connectivity probe of the selected provider and hand it
    /// to the run loop.
    ///
    /// Returns `true` when there is something for the run loop to do, so the
    /// caller can raise [`SettingsAction::RunProviderProbe`] rather than
    /// guessing. A precondition failure sets the banner here and returns
    /// `false`: nothing was asked of the network, so nothing needs the run
    /// loop.
    fn begin_provider_test(&mut self) -> bool {
        self.begin_provider_probe(ProbeKind::Connectivity)
    }

    /// `m`: Phase 9D line 2, and manual by construction — this runs because
    /// a key was pressed and there is no other caller.
    fn begin_provider_model_refresh(&mut self) -> bool {
        let Some(row) = self.providers.get(self.selected_provider) else {
            return false;
        };
        // Asked before anything else: a provider with no model list must
        // produce a sentence, not a failed request against a guessed path.
        if let Err(why) = model_discovery_availability(&row.name, &row.config) {
            let name = row.name.clone();
            self.provider_notice =
                Some((name, ProviderNotice::Models(ModelRefresh::NotOffered(why))));
            return false;
        }
        self.begin_provider_probe(ProbeKind::ModelRefresh)
    }

    fn begin_provider_probe(&mut self, kind: ProbeKind) -> bool {
        let Some(row) = self.providers.get(self.selected_provider) else {
            return false;
        };
        let name = row.name.clone();

        // A second press while one is already running would open a second
        // socket and leave two results racing for one banner. Refused, and
        // said out loud rather than ignored — a key that silently does
        // nothing is indistinguishable from a frozen screen.
        if row.activity.is_some() {
            self.provider_notice = Some((
                name.clone(),
                ProviderNotice::Reachability(ReachabilityCheck::Failed(format!(
                    "a request for `{name}` is already running; wait for it to come back"
                ))),
            ));
            return false;
        }

        // The store a launch would actually use, not just the environment: a
        // key the user put in the Keychain is a key this check must count as
        // present, or `t` would report a provider as unusable that launches
        // perfectly well.
        let intent =
            match plan_provider_probe(&name, &row.config, kind, &PreferNativeSecretStore::detect())
            {
                Ok(intent) => intent,
                Err(why) => {
                    self.provider_notice = Some(match kind {
                        ProbeKind::Connectivity => (
                            name,
                            ProviderNotice::Reachability(ReachabilityCheck::Failed(why)),
                        ),
                        ProbeKind::ModelRefresh => {
                            (name, ProviderNotice::Models(ModelRefresh::Failed(why)))
                        }
                    });
                    return false;
                }
            };

        let endpoint = probe_endpoint(&intent);
        self.provider_notice = Some(match kind {
            ProbeKind::Connectivity => (
                name.clone(),
                ProviderNotice::Reachability(ReachabilityCheck::InFlight {
                    protocol: intent.protocol.slug(),
                    base_url: intent.base_url.clone(),
                    endpoint,
                }),
            ),
            ProbeKind::ModelRefresh => (
                name.clone(),
                ProviderNotice::Models(ModelRefresh::InFlight { endpoint }),
            ),
        });
        if let Some(row) = self.providers.get_mut(self.selected_provider) {
            row.activity = Some(kind);
        }
        self.pending_probe = Some(intent);
        true
    }

    /// A finished probe, back from the run loop.
    ///
    /// The row's in-flight marker is cleared whatever the outcome, including
    /// for a provider the user has since deleted — the lookup simply finds
    /// nothing and the banner still tells them what happened to the request
    /// they started.
    pub(super) fn apply_probe_result(&mut self, result: ProviderProbeResult) {
        if let Some(row) = self
            .providers
            .iter_mut()
            .find(|row| row.name == result.provider)
        {
            row.activity = None;
            if let Some(catalogue) = result.catalogue {
                row.models = Some(catalogue);
            }
        }
        self.provider_notice = Some((result.provider, result.notice));
    }

    pub(super) fn take_probe_intent(&mut self) -> Option<ProviderProbeIntent> {
        self.pending_probe.take()
    }

    /// Whether any provider row has a request on the wire.
    pub(super) fn any_probe_in_flight(&self) -> bool {
        self.providers.iter().any(|row| row.activity.is_some())
    }

    fn handle_provider_input_key(&mut self, key: KeyEvent) -> SettingsAction {
        match key.code {
            KeyCode::Esc => {
                self.provider_input = None;
                SettingsAction::Redraw
            }
            KeyCode::Enter => {
                // A credential is not applied here: writing one to the OS
                // store is I/O, so the input is left standing and the run
                // loop takes the value out of it.
                if self
                    .provider_input
                    .as_ref()
                    .is_some_and(|input| input.purpose.is_secret())
                {
                    let empty = self
                        .provider_input
                        .as_ref()
                        .is_some_and(|input| input.buffer.trim().is_empty());
                    if empty {
                        if let Some(input) = self.provider_input.as_mut() {
                            input.error = Some(
                                "a credential needs a value; press Esc to leave it unchanged"
                                    .to_owned(),
                            );
                        }
                        return SettingsAction::Redraw;
                    }
                    return SettingsAction::StoreCredential;
                }
                self.confirm_provider_input();
                SettingsAction::Redraw
            }
            KeyCode::Backspace => {
                if let Some(input) = self.provider_input.as_mut() {
                    input.buffer.pop();
                    input.error = None;
                }
                SettingsAction::Redraw
            }
            KeyCode::Char(c) => {
                if let Some(input) = self.provider_input.as_mut() {
                    input.buffer.push(c);
                    input.error = None;
                }
                SettingsAction::Redraw
            }
            _ => SettingsAction::None,
        }
    }

    /// Apply the typed text for whichever [`ProviderInputPurpose`] is
    /// active. On success this closes the input (`self.provider_input =
    /// None`, already true from the `take()` below unless a validation
    /// failure re-opens it with an error attached); on failure it re-opens
    /// the same input with `error` set, so Esc still cancels and the buffer
    /// is not lost.
    fn confirm_provider_input(&mut self) {
        let Some(input) = self.provider_input.take() else {
            return;
        };
        let typed = input.buffer.trim().to_owned();
        match input.purpose {
            ProviderInputPurpose::NewName => {
                if typed.is_empty() {
                    self.provider_input = Some(ProviderTextInput {
                        purpose: ProviderInputPurpose::NewName,
                        buffer: input.buffer,
                        error: Some("a provider needs a name".to_owned()),
                    });
                    return;
                }
                if self.providers.iter().any(|row| row.name == typed) {
                    self.provider_input = Some(ProviderTextInput {
                        purpose: ProviderInputPurpose::NewName,
                        buffer: input.buffer,
                        error: Some(format!("a provider named `{typed}` already exists")),
                    });
                    return;
                }
                self.provider_input = Some(ProviderTextInput {
                    purpose: ProviderInputPurpose::NewTemplate { name: typed },
                    buffer: String::new(),
                    error: None,
                });
            }
            ProviderInputPurpose::NewTemplate { name } => {
                if crate::provider::template(&typed).is_none() {
                    let known: Vec<String> = crate::provider::templates()
                        .into_iter()
                        .map(|p| p.name)
                        .collect();
                    self.provider_input = Some(ProviderTextInput {
                        purpose: ProviderInputPurpose::NewTemplate { name },
                        buffer: input.buffer,
                        error: Some(format!(
                            "`{typed}` is not a known provider template; known templates are: {}",
                            known.join(", ")
                        )),
                    });
                    return;
                }
                let config = ProviderConfig::new(typed);
                self.providers
                    .push(ProviderRow::new(name.clone(), config.clone(), Layer::User));
                self.providers.sort_by(|a, b| a.name.cmp(&b.name));
                self.selected_provider = self
                    .providers
                    .iter()
                    .position(|row| row.name == name)
                    .unwrap_or(0);
                self.provider_edits.insert(name, Some(config));
            }
            ProviderInputPurpose::EditBaseUrl { name } => {
                let value = (!typed.is_empty()).then_some(typed);
                if let Some(row) = self.providers.iter_mut().find(|row| row.name == name) {
                    row.config.set_base_url(value);
                    row.layer = Layer::User;
                    self.provider_edits.insert(name, Some(row.config.clone()));
                }
            }
            ProviderInputPurpose::EditCredentialEnv { name } => {
                let names: Vec<String> = typed
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
                if let Some(row) = self.providers.iter_mut().find(|row| row.name == name) {
                    row.config.set_credential_env(names);
                    row.layer = Layer::User;
                    self.provider_edits.insert(name, Some(row.config.clone()));
                }
            }
            ProviderInputPurpose::EditFreeModels { name } => {
                let names: Vec<String> = typed
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
                if let Some(row) = self.providers.iter_mut().find(|row| row.name == name) {
                    row.config.set_free_models(names);
                    row.layer = Layer::User;
                    self.provider_edits.insert(name, Some(row.config.clone()));
                }
            }
            // Never reached: `handle_provider_input_key` answers Enter on a
            // credential field with `SettingsAction::StoreCredential` and
            // leaves the input standing, because storing one is I/O. Written
            // as a no-op rather than a panic so that a future path into here
            // discards the value instead of printing a backtrace next to it.
            ProviderInputPurpose::SetCredential { .. } => {}
        }
    }

    // -------------------------------------------------------------
    // Launch profiles
    // -------------------------------------------------------------

    fn start_edit_profile_model(&mut self) {
        let Some(row) = self.profiles.get(self.selected_profile) else {
            return;
        };
        self.profile_input = Some(ProfileTextInput {
            purpose: ProfileInputPurpose::EditModel {
                name: row.name.clone(),
            },
            buffer: row.config.model().unwrap_or_default().to_owned(),
            error: None,
        });
    }

    fn start_edit_profile_backend(&mut self) {
        let Some(row) = self.profiles.get(self.selected_profile) else {
            return;
        };
        let buffer = match row.config.backend() {
            ProfileBackend::Native => "native".to_owned(),
            ProfileBackend::DirectProvider { provider } => provider.clone(),
            ProfileBackend::GlasshouseGateway => String::new(),
        };
        self.profile_input = Some(ProfileTextInput {
            purpose: ProfileInputPurpose::EditBackend {
                name: row.name.clone(),
            },
            buffer,
            error: None,
        });
    }

    fn start_duplicate_profile(&mut self) {
        let Some(row) = self.profiles.get(self.selected_profile) else {
            return;
        };
        self.profile_input = Some(ProfileTextInput {
            purpose: ProfileInputPurpose::Duplicate {
                source: row.name.clone(),
            },
            buffer: String::new(),
            error: None,
        });
    }

    fn toggle_selected_profile(&mut self) {
        let Some(row) = self.profiles.get_mut(self.selected_profile) else {
            return;
        };
        row.config.set_enabled(!row.config.enabled());
        row.layer = Layer::User;
        self.profile_edits
            .insert(row.name.clone(), Some(row.config.clone()));
    }

    fn remove_selected_profile(&mut self) {
        if self.selected_profile >= self.profiles.len() {
            return;
        }
        let row = self.profiles.remove(self.selected_profile);
        self.profile_edits.insert(row.name, None);
        self.selected_profile = self
            .selected_profile
            .min(self.profiles.len().saturating_sub(1));
    }

    fn cycle_selected_profile_approval(&mut self) {
        let Some(row) = self.profiles.get_mut(self.selected_profile) else {
            return;
        };
        let next = match row.config.approval() {
            ProfileApproval::Default => ProfileApproval::AutomaticReview,
            ProfileApproval::AutomaticReview => ProfileApproval::Bypass,
            ProfileApproval::Bypass => ProfileApproval::Default,
        };
        row.config.set_approval(next);
        row.layer = Layer::User;
        self.profile_edits
            .insert(row.name.clone(), Some(row.config.clone()));
    }

    fn handle_profile_input_key(&mut self, key: KeyEvent) -> SettingsAction {
        match key.code {
            KeyCode::Esc => {
                self.profile_input = None;
                SettingsAction::Redraw
            }
            KeyCode::Enter => {
                self.confirm_profile_input();
                SettingsAction::Redraw
            }
            KeyCode::Backspace => {
                if let Some(input) = self.profile_input.as_mut() {
                    input.buffer.pop();
                    input.error = None;
                }
                SettingsAction::Redraw
            }
            KeyCode::Char(c) => {
                if let Some(input) = self.profile_input.as_mut() {
                    input.buffer.push(c);
                    input.error = None;
                }
                SettingsAction::Redraw
            }
            _ => SettingsAction::None,
        }
    }

    /// Apply the typed text for whichever [`ProfileInputPurpose`] is active
    /// — see [`SettingsState::confirm_provider_input`]'s doc for the
    /// success/failure shape this mirrors.
    fn confirm_profile_input(&mut self) {
        let Some(input) = self.profile_input.take() else {
            return;
        };
        let typed = input.buffer.trim().to_owned();
        match input.purpose {
            ProfileInputPurpose::NewName => {
                if typed.is_empty() {
                    self.profile_input = Some(ProfileTextInput {
                        purpose: ProfileInputPurpose::NewName,
                        buffer: input.buffer,
                        error: Some("a launch profile needs a name".to_owned()),
                    });
                    return;
                }
                if self.profiles.iter().any(|row| row.name == typed) {
                    self.profile_input = Some(ProfileTextInput {
                        purpose: ProfileInputPurpose::NewName,
                        buffer: input.buffer,
                        error: Some(format!("a launch profile named `{typed}` already exists")),
                    });
                    return;
                }
                self.profile_input = Some(ProfileTextInput {
                    purpose: ProfileInputPurpose::NewHarness { name: typed },
                    buffer: String::new(),
                    error: None,
                });
            }
            ProfileInputPurpose::NewHarness { name } => {
                let Some(harness) = known_launch_harnesses().find(|id| id.slug() == typed) else {
                    let known: Vec<&str> = known_launch_harnesses().map(|id| id.slug()).collect();
                    self.profile_input = Some(ProfileTextInput {
                        purpose: ProfileInputPurpose::NewHarness { name },
                        buffer: input.buffer,
                        error: Some(format!(
                            "`{typed}` is not a harness Glasshouse knows; known harnesses are: {}",
                            known.join(", ")
                        )),
                    });
                    return;
                };
                let config = ProfileConfig::new(harness);
                self.profiles.push(ProfileRow {
                    name: name.clone(),
                    config: config.clone(),
                    layer: Layer::User,
                });
                self.profiles.sort_by(|a, b| a.name.cmp(&b.name));
                self.selected_profile = self
                    .profiles
                    .iter()
                    .position(|row| row.name == name)
                    .unwrap_or(0);
                self.profile_edits.insert(name, Some(config));
            }
            ProfileInputPurpose::EditModel { name } => {
                let value = (!typed.is_empty()).then_some(typed);
                if let Some(row) = self.profiles.iter_mut().find(|row| row.name == name) {
                    row.config.set_model(value);
                    row.layer = Layer::User;
                    self.profile_edits.insert(name, Some(row.config.clone()));
                }
            }
            ProfileInputPurpose::EditBackend { name } => {
                let backend = if typed.is_empty() || typed.eq_ignore_ascii_case("native") {
                    Some(ProfileBackend::Native)
                } else if self.providers.iter().any(|row| row.name == typed) {
                    Some(ProfileBackend::DirectProvider {
                        provider: typed.clone(),
                    })
                } else {
                    None
                };
                let Some(backend) = backend else {
                    self.profile_input = Some(ProfileTextInput {
                        purpose: ProfileInputPurpose::EditBackend { name },
                        buffer: input.buffer,
                        error: Some(format!(
                            "`{typed}` is not `native` or a configured provider name"
                        )),
                    });
                    return;
                };
                if let Some(row) = self.profiles.iter_mut().find(|row| row.name == name) {
                    row.config.set_backend(backend);
                    row.layer = Layer::User;
                    self.profile_edits.insert(name, Some(row.config.clone()));
                }
            }
            ProfileInputPurpose::Duplicate { source } => {
                if typed.is_empty() {
                    self.profile_input = Some(ProfileTextInput {
                        purpose: ProfileInputPurpose::Duplicate { source },
                        buffer: input.buffer,
                        error: Some("a new profile needs a name".to_owned()),
                    });
                    return;
                }
                if self.profiles.iter().any(|row| row.name == typed) {
                    self.profile_input = Some(ProfileTextInput {
                        purpose: ProfileInputPurpose::Duplicate { source },
                        buffer: input.buffer,
                        error: Some(format!("a launch profile named `{typed}` already exists")),
                    });
                    return;
                }
                let Some(source_row) = self.profiles.iter().find(|row| row.name == source) else {
                    return;
                };
                let config = source_row.config.clone();
                self.profiles.push(ProfileRow {
                    name: typed.clone(),
                    config: config.clone(),
                    layer: Layer::User,
                });
                self.profiles.sort_by(|a, b| a.name.cmp(&b.name));
                self.selected_profile = self
                    .profiles
                    .iter()
                    .position(|row| row.name == typed)
                    .unwrap_or(0);
                self.profile_edits.insert(typed, Some(config));
            }
        }
    }

    // -------------------------------------------------------------
    // Routing
    // -------------------------------------------------------------

    fn start_routing_input(&mut self, purpose: RoutingInputPurpose) {
        let buffer = match purpose {
            RoutingInputPurpose::Model => match &self.routing.model {
                RoutingModelChoice::Automatic => "automatic".to_owned(),
                RoutingModelChoice::Deterministic => "deterministic".to_owned(),
                RoutingModelChoice::Pinned { provider, model } => {
                    format!("{provider}:{model}")
                }
            },
            RoutingInputPurpose::MaxLatency => self.routing.max_latency.get().to_string(),
            RoutingInputPurpose::MaxCost => format_usd(self.routing.max_cost),
            RoutingInputPurpose::PremiumReserve => self.routing.premium_reserve.get().to_string(),
            RoutingInputPurpose::FreeOrder => format_free_resource_list(&self.routing.free_order),
            RoutingInputPurpose::FreeDisabled => {
                format_free_resource_list(&self.routing.free_disabled)
            }
            RoutingInputPurpose::FreePin => self
                .routing
                .free_pin
                .as_ref()
                .map(format_free_resource_ref)
                .unwrap_or_default(),
        };
        self.routing_input = Some(RoutingTextInput {
            purpose,
            buffer,
            error: None,
        });
    }

    fn handle_routing_input_key(&mut self, key: KeyEvent) -> SettingsAction {
        match key.code {
            KeyCode::Esc => {
                self.routing_input = None;
                SettingsAction::Redraw
            }
            KeyCode::Enter => {
                self.confirm_routing_input();
                SettingsAction::Redraw
            }
            KeyCode::Backspace => {
                if let Some(input) = self.routing_input.as_mut() {
                    input.buffer.pop();
                    input.error = None;
                }
                SettingsAction::Redraw
            }
            KeyCode::Char(c) => {
                if let Some(input) = self.routing_input.as_mut() {
                    input.buffer.push(c);
                    input.error = None;
                }
                SettingsAction::Redraw
            }
            _ => SettingsAction::None,
        }
    }

    fn confirm_routing_input(&mut self) {
        let Some(input) = self.routing_input.take() else {
            return;
        };
        let typed = input.buffer.trim();
        let result = match input.purpose {
            RoutingInputPurpose::Model => self.apply_routing_model(typed),
            RoutingInputPurpose::MaxLatency => typed
                .parse::<u32>()
                .map_err(|_| "latency must be a whole number of milliseconds".to_owned())
                .and_then(|value| RouterLatencyMs::try_from(value).map_err(|err| err.to_string()))
                .map(|value| {
                    self.routing.max_latency = value;
                    self.routing.max_latency_layer = Layer::User;
                    self.routing_edit.max_latency = Some(value);
                }),
            RoutingInputPurpose::MaxCost => parse_usd_micro(typed).map(|value| {
                self.routing.max_cost = value;
                self.routing.max_cost_layer = Layer::User;
                self.routing_edit.max_cost = Some(value);
            }),
            RoutingInputPurpose::PremiumReserve => typed
                .parse::<u16>()
                .map_err(|_| "reserve must be a whole-number percentage".to_owned())
                .and_then(|value| {
                    PremiumReservePercent::try_from(value).map_err(|err| err.to_string())
                })
                .map(|value| {
                    self.routing.premium_reserve = value;
                    self.routing.premium_reserve_layer = Layer::User;
                    self.routing_edit.premium_reserve = Some(value);
                }),
            RoutingInputPurpose::FreeOrder => parse_free_resource_list(typed).map(|value| {
                self.routing.free_order = value.clone();
                self.routing.free_order_layer = Layer::User;
                self.routing_edit.free_order = Some(value);
            }),
            RoutingInputPurpose::FreeDisabled => parse_free_resource_list(typed).map(|value| {
                self.routing.free_disabled = value.clone();
                self.routing.free_disabled_layer = Layer::User;
                self.routing_edit.free_disabled = Some(value);
            }),
            RoutingInputPurpose::FreePin => {
                let pin = if typed.is_empty() {
                    Ok(None)
                } else {
                    parse_free_resource_ref(typed).map(Some)
                };
                pin.map(|value| {
                    self.routing.free_pin = value.clone();
                    self.routing.free_pin_layer = Layer::User;
                    self.routing_edit.free_pin = Some(value);
                })
            }
        };
        if let Err(error) = result {
            self.routing_input = Some(RoutingTextInput {
                purpose: input.purpose,
                buffer: input.buffer,
                error: Some(error),
            });
        }
    }

    fn apply_routing_model(&mut self, typed: &str) -> Result<(), String> {
        let choice = if typed.eq_ignore_ascii_case("automatic") {
            RoutingModelChoice::Automatic
        } else if typed.eq_ignore_ascii_case("deterministic") {
            RoutingModelChoice::Deterministic
        } else {
            let Some((provider, model)) = typed.split_once(':') else {
                return Err("use `automatic`, `deterministic`, or `provider:model`".to_owned());
            };
            let provider = provider.trim();
            let model = model.trim();
            if provider.is_empty() || model.is_empty() {
                return Err("a pinned choice needs both a provider and model".to_owned());
            }
            if !self
                .routing
                .configured_providers
                .iter()
                .any(|configured| configured == provider)
            {
                return Err(format!("`{provider}` is not a configured provider"));
            }
            RoutingModelChoice::Pinned {
                provider: provider.to_owned(),
                model: model.to_owned(),
            }
        };
        self.routing.model = choice.clone();
        self.routing.model_layer = Layer::User;
        self.routing_edit.model = Some(choice);
        Ok(())
    }

    fn handle_path_input_key(&mut self, key: KeyEvent) -> SettingsAction {
        match key.code {
            KeyCode::Esc => {
                self.path_input = None;
                SettingsAction::Redraw
            }
            KeyCode::Enter => {
                let typed = {
                    let input = self.path_input.as_ref().expect("checked above");
                    PathBuf::from(input.buffer.trim())
                };
                match exec::resolve_explicit(&typed) {
                    Ok(resolved) => {
                        let index = self.selected_harness;
                        if let Some(row) = self.harnesses.get_mut(index) {
                            let path = resolved.path().to_path_buf();
                            row.executable = Some(path.clone());
                            row.executable_layer = Some(Layer::User);
                            self.edits.entry(row.id).or_default().executable = Some(Some(path));
                        }
                        self.path_input = None;
                    }
                    Err(err) => {
                        if let Some(input) = self.path_input.as_mut() {
                            input.error = Some(err.to_string());
                        }
                    }
                }
                SettingsAction::Redraw
            }
            KeyCode::Backspace => {
                if let Some(input) = self.path_input.as_mut() {
                    input.buffer.pop();
                    input.error = None;
                }
                SettingsAction::Redraw
            }
            KeyCode::Char(c) => {
                if let Some(input) = self.path_input.as_mut() {
                    input.buffer.push(c);
                    input.error = None;
                }
                SettingsAction::Redraw
            }
            _ => SettingsAction::None,
        }
    }
}
