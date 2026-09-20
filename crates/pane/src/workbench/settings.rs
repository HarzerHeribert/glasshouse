//! Direct local preferences over Pane's existing typed, conflict-aware store.
use crate::settings::{Kind, Loaded, Scope, SettingSpec, Snapshot, Store};
use crate::tui::ScreenState;
use std::path::PathBuf;

pub const CATEGORIES: [&str; 6] = [
    "Workspace",
    "Display",
    "Little helpers",
    "Models & accounts",
    "Subagents",
    "Advanced",
];
pub struct Preferences {
    pub scope: Scope,
    pub category: usize,
    pub selected: usize,
    pub query: String,
    pub editing: Option<(String, String)>,
    pub notice: String,
    pub snapshot: Snapshot,
    pub loaded: Loaded,
    pub path: PathBuf,
    root: PathBuf,
    global: Option<PathBuf>,
    profile: Option<String>,
    undo: Option<Vec<(String, Option<String>)>>,
}
impl Preferences {
    pub fn open(s: &ScreenState) -> Result<Self, String> {
        Self::with_global(s, crate::project::workflows::user_directory())
    }
    /// Tests and embedded hosts can supply a global directory without process-wide environment edits.
    pub fn with_global(s: &ScreenState, global: Option<PathBuf>) -> Result<Self, String> {
        let root = s
            .settings_root
            .clone()
            .ok_or("Settings require a project root.")?;
        let store = Store::with_global(&root, global.clone())?;
        Ok(Self {
            scope: Scope::Local,
            category: 0,
            selected: 0,
            query: String::new(),
            editing: None,
            notice: String::new(),
            snapshot: store.read(Scope::Local)?,
            loaded: store.load(s.settings_profile.as_deref())?,
            path: store.path(Scope::Local).to_path_buf(),
            root,
            global,
            profile: s.settings_profile.clone(),
            undo: None,
        })
    }
    pub fn rows(&self) -> Vec<&'static SettingSpec> {
        crate::settings::specs()
            .iter()
            .filter(|spec| {
                if !self.query.is_empty() {
                    let q = self.query.to_lowercase();
                    return format!("{} {} {}", spec.key, spec.label, spec.description)
                        .to_lowercase()
                        .contains(&q);
                }
                let k = spec.key;
                match self.category {
                    0 => ["session.mode", "session.effort", "permissions.mode"].contains(&k),
                    1 => k.starts_with("ui."),
                    2 => [
                        "helpers.enabled",
                        "helpers.completion",
                        "helpers.preflight",
                        "helpers.completion_check",
                        "helpers.effort.find",
                        "helpers.effort.reduce",
                        "helpers.effort.check",
                    ]
                    .contains(&k),
                    3 => ["model.parent", "helpers.model"].contains(&k),
                    4 => {
                        k == "agents.mode"
                            || k == "agents.model"
                            || (k.starts_with("agents.slots.") && k.ends_with(".model"))
                    }
                    _ => !spec.basic,
                }
            })
            .collect()
    }
    pub fn effective(&self, key: &str) -> String {
        show(get(&self.loaded.values, key))
    }
    pub fn saved(&self, key: &str) -> Option<String> {
        get(&self.snapshot.values, key).map(|v| show(Some(v)))
    }
    pub fn origin(&self, key: &str) -> &str {
        self.loaded
            .origins
            .get(key)
            .map(String::as_str)
            .unwrap_or("built-in")
    }
    pub fn choices(spec: &SettingSpec) -> Vec<String> {
        if spec.key == "agents.mode" {
            return vec!["off".into(), "pinned".into(), "roster".into()];
        }
        if spec.kind == Kind::Bool {
            return vec!["false".into(), "true".into()];
        }
        spec.choices.iter().map(|s| s.to_string()).collect()
    }
    pub fn save(
        &mut self,
        key: &str,
        value: Option<String>,
        s: &mut ScreenState,
    ) -> Result<(), String> {
        let store = Store::with_global(&self.root, self.global.clone())?;
        let mut edits = vec![(key.to_string(), value.clone())];
        if key == "agents.model" && value.is_some() {
            edits.push(("agents.mode".into(), Some("pinned".into())));
        }
        if key == "agents.mode" && value.as_deref().is_some_and(|v| v != "pinned") {
            edits.push(("agents.model".into(), None));
        }
        if key.starts_with("agents.slots.") && key.ends_with(".model") && value.is_none() {
            edits.push((key.replace(".model", ".effort"), None));
            if self.loaded.config.agents.mode == crate::config::AgentsMode::Roster
                && self.loaded.config.agents.slots.len() == 1
            {
                edits.push(("agents.mode".into(), Some("off".into())));
            }
        }
        let previous = edits
            .iter()
            .map(|(key, _)| (key.clone(), self.saved(key)))
            .collect();
        store.save(self.scope, &self.snapshot, &edits)?;
        self.undo = Some(previous);
        self.reload()?;
        // Apply only the edited presentation field: an unrelated save must
        // not erase a live /motion or /sidebar override.
        let mut resolved = ScreenState::default();
        crate::settings_session::presentation(&mut resolved, &self.loaded.values);
        match key {
            "ui.theme" => s.theme = resolved.theme,
            "ui.reduced_motion" => s.reduced_motion = resolved.reduced_motion,
            "ui.statusline" => s.status_line = resolved.status_line,
            "ui.sidebar" => s.sidebar = resolved.sidebar,
            _ => {}
        }
        self.notice = if key.starts_with("ui.") {
            format!(
                "Saved · active presentation uses {} · Undo available",
                self.origin(key)
            )
        } else {
            "Saved for a new session · running authority/model unchanged · Undo available".into()
        };
        if self.profile.is_some() {
            self.notice
                .push_str(" · selected profile may override this base scope");
        }
        Ok(())
    }
    pub fn cycle(&mut self, forward: bool, s: &mut ScreenState) -> Result<(), String> {
        let Some(spec) = self.rows().get(self.selected).copied() else {
            return Ok(());
        };
        let choices = Self::choices(spec);
        if choices.is_empty() {
            self.editing = Some((spec.key.into(), self.effective(spec.key)));
            return Ok(());
        }
        let n = choices.len();
        let old = choices
            .iter()
            .position(|v| *v == self.effective(spec.key))
            .unwrap_or(0);
        let i = if forward {
            (old + 1) % n
        } else {
            (old + n - 1) % n
        };
        // Safety-relevant choices need an explicit field confirmation, never arrow rollover.
        if spec.key.starts_with("permissions.") || spec.key == "agents.mode" {
            self.editing = Some((spec.key.into(), choices[i].clone()));
            return Ok(());
        }
        self.save(spec.key, Some(choices[i].clone()), s)
    }
    pub fn undo(&mut self, s: &mut ScreenState) -> Result<(), String> {
        if let Some(previous) = self.undo.clone() {
            let store = Store::with_global(&self.root, self.global.clone())?;
            store.save(self.scope, &self.snapshot, &previous)?;
            self.reload()?;
            let mut resolved = ScreenState::default();
            crate::settings_session::presentation(&mut resolved, &self.loaded.values);
            for (key, _) in previous {
                match key.as_str() {
                    "ui.theme" => s.theme = resolved.theme,
                    "ui.reduced_motion" => s.reduced_motion = resolved.reduced_motion,
                    "ui.statusline" => s.status_line = resolved.status_line,
                    "ui.sidebar" => s.sidebar = resolved.sidebar,
                    _ => {}
                }
            }
            self.undo = None;
            self.notice =
                "Last setting restored. Runtime changes still require a new session.".into();
        }
        Ok(())
    }
    pub fn switch_scope(&mut self) -> Result<(), String> {
        let next = if self.scope == Scope::Local {
            Scope::Global
        } else {
            Scope::Local
        };
        let store = Store::with_global(&self.root, self.global.clone())?;
        let snapshot = store.read(next)?;
        self.scope = next;
        self.snapshot = snapshot;
        self.path = store.path(next).to_path_buf();
        self.undo = None;
        self.editing = None;
        self.notice.clear();
        Ok(())
    }
    fn reload(&mut self) -> Result<(), String> {
        let store = Store::with_global(&self.root, self.global.clone())?;
        self.snapshot = store.read(self.scope)?;
        self.loaded = store.load(self.profile.as_deref())?;
        Ok(())
    }
}
fn get<'a>(v: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    key.split('.').try_fold(v, |v, key| v.get(key))
}
fn show(v: Option<&toml::Value>) -> String {
    v.map(|v| {
        v.as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| v.to_string())
    })
    .unwrap_or_else(|| "unset".into())
}
