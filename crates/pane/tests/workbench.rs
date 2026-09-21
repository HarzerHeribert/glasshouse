//! Acceptance of the new live renderer, input reducer and native settings store.
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use pane::{
    contract::{Block, Conversation, Message, Role, ServedBy},
    helpers::{HelperOutcome, HelperRecord, HelperUsage},
    tui::{
        Activity, CellError, CellView, ModelGroup, Notebook, Panel, ScreenState, Theme, TierModels,
    },
    workbench::{self, Action, CellTab, Document, Effect, Navigator, Preferences, Tone, Workbench},
};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, style::Color};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
fn fixture() -> (Conversation, Notebook, ScreenState) {
    let mut m = Message::text(
        Role::Assistant,
        "I will inspect the motion guard before changing it.",
    );
    m.content.push(Block::ToolUse{id:"call-1".into(),name:"execute_cell".into(),input:serde_json::json!({"code":"const result = await checks.run(\"tests\");\nprint(result);"})});
    (
        Conversation {
            system: String::new(),
            messages: vec![
                Message::text(
                    Role::User,
                    "Respect reduced motion and the terminal background.",
                ),
                m,
            ],
        },
        Notebook {
            cells: vec![CellView {
                executed_source: Some(
                    "const result = await checks.run(\"tests\");\nprint(result);".into(),
                ),
                execution: Some("checks.run · completed".into()),
                output: Some("99 / 99 tests passed".into()),
                stdout: Some("Compiling dependency graph".into()),
                changes: Some("--- a/view.rs\n+++ b/view.rs\n@@ -1 +1 @@\n-old();\n+new();".into()),
                helpers: vec![HelperRecord {
                    helper: "reduce".into(),
                    verb: "reducing".into(),
                    asked: "Retain failures and source locations".into(),
                    outcome: HelperOutcome {
                        text: "No failing tests in the supplied output".into(),
                        ok: true,
                        elapsed_ms: 1200,
                        ..Default::default()
                    },
                    usage: HelperUsage {
                        model: "fixture-helper".into(),
                        ..Default::default()
                    },
                    looked: vec!["prepare failure windows".into()],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        },
        ScreenState {
            model: Some("fixture-main".into()),
            project: Some("test-project".into()),
            activity: Activity::Complete,
            ..Default::default()
        },
    )
}
fn draw(
    c: &Conversation,
    n: &Notebook,
    s: &ScreenState,
    u: &mut Workbench,
    w: u16,
    h: u16,
) -> Buffer {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| workbench::render(f, c, n, s, &ServedBy::default(), u))
        .unwrap();
    t.backend().buffer().clone()
}
fn text(b: &Buffer) -> String {
    (0..b.area.height)
        .map(|y| {
            (0..b.area.width)
                .map(|x| b[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn key(u: &mut Workbench, s: &mut ScreenState, n: &Notebook, k: KeyCode) -> Effect {
    u.event(
        &Event::Key(KeyEvent::new(k, KeyModifiers::NONE)),
        s,
        n,
        false,
    )
}
fn mouse(
    u: &mut Workbench,
    s: &mut ScreenState,
    n: &Notebook,
    kind: MouseEventKind,
    x: u16,
    y: u16,
) -> Effect {
    u.event(
        &Event::Mouse(MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }),
        s,
        n,
        false,
    )
}
fn click(u: &mut Workbench, s: &mut ScreenState, n: &Notebook, a: Action) -> Effect {
    let (r, _) = u
        .geometry
        .hits
        .iter()
        .find(|(_, v)| *v == a)
        .unwrap()
        .clone();
    mouse(u, s, n, MouseEventKind::Down(MouseButton::Left), r.x, r.y);
    mouse(u, s, n, MouseEventKind::Up(MouseButton::Left), r.x, r.y)
}
fn doc(c: &Conversation, n: &Notebook, s: &ScreenState, u: &Workbench) -> Document {
    Document::build(c, n, s, u, 100)
}
fn words(d: &Document) -> String {
    d.rows
        .iter()
        .map(|r| r.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "pane-workbench-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn prefs() -> (Temp, ScreenState, Preferences) {
    let t = Temp::new();
    let s = ScreenState {
        settings_root: Some(t.0.clone()),
        ..Default::default()
    };
    let p = Preferences::with_global(&s, Some(t.0.join("user"))).unwrap();
    (t, s, p)
}
fn navigator() -> Navigator {
    let group = |provider: &str, available: bool, models: Vec<&str>| ModelGroup {
        provider: provider.into(),
        account: format!("{provider}-subscription"),
        scope: "subscription".into(),
        models: models.into_iter().map(str::to_string).collect(),
        selectable: Some(available),
        unavailable_reason: (!available).then(|| "No credential configured".into()),
        connect: None,
    };
    let panel = Panel::models(
        "Models",
        vec![
            group("A", true, vec!["fixture-main", "fixture-helper"]),
            group("OpenRouter", false, vec!["unavailable-model"]),
        ],
        TierModels {
            parent: "fixture-main".into(),
            helper: Some("fixture-helper".into()),
            subagent: None,
        },
    );
    Navigator::from_panel(&panel).unwrap()
}
#[test]
fn code_public_explanation_and_results_remain_readable() {
    let (c, n, s) = fixture();
    let d = doc(&c, &n, &s, &Workbench::default());
    for t in [
        "const result",
        "inspect the motion guard",
        "99 / 99",
        "No failing tests",
    ] {
        assert!(words(&d).contains(t));
    }
    assert!(d.rows.iter().any(|r| {
        r.text.contains("99 / 99")
            && r.spans
                .iter()
                .any(|(t, tone)| t.contains("99 / 99") && *tone == Tone::Normal)
    }));
}
#[test]
fn collapsed_cell_keeps_helper_contributions() {
    let (c, n, s) = fixture();
    let mut u = Workbench::default();
    u.collapsed.insert(1);
    let t = words(&doc(&c, &n, &s, &u));
    assert!(!t.contains("const result"));
    assert!(t.contains("No failing tests"));
}
#[test]
fn local_notices_are_not_model_conversation() {
    let (c, n, mut s) = fixture();
    s.note("SETTINGS INTERNAL MESSAGE");
    let mut u = Workbench::default();
    // A notice is kept where it happened, so it can be scrolled back to --
    // and it is marked and muted, never drawn as something the model said.
    let d = doc(&c, &n, &s, &u);
    let row = d
        .rows
        .iter()
        .find(|r| r.text.contains("SETTINGS INTERNAL"))
        .expect("the notice is in the local document");
    assert!(row.text.trim_start().starts_with('·'), "{:?}", row.text);
    assert!(
        row.spans
            .iter()
            .any(|(t, tone)| t.contains("SETTINGS INTERNAL") && *tone == Tone::Muted),
        "{:?}",
        row.spans
    );
    assert!(text(&draw(&c, &n, &s, &mut u, 100, 40)).contains("SETTINGS INTERNAL"));
    u.activity = true;
    assert!(text(&draw(&c, &n, &s, &mut u, 100, 40)).contains("SETTINGS INTERNAL"));
}
#[test]
fn final_return_visible_without_an_extra_message_and_not_duplicated() {
    let (mut c, mut n, s) = fixture();
    n.cells[0].returned = Some("The motion guard is fixed.".into());
    assert!(words(&doc(&c, &n, &s, &Workbench::default())).contains("The motion guard is fixed."));
    c.messages
        .push(Message::text(Role::Assistant, "The motion guard is fixed."));
    assert_eq!(
        words(&doc(&c, &n, &s, &Workbench::default()))
            .matches("The motion guard is fixed.")
            .count(),
        1
    );
}
#[test]
fn runtime_feedback_not_confused_with_the_next_human_message() {
    let (mut c, mut n, s) = fixture();
    n.cells[0].answered = true;
    c.messages.push(Message::tool_result(
        "call-1",
        "PRIVATE PROTOCOL NOISE",
        false,
    ));
    c.messages
        .push(Message::text(Role::User, "Now inspect the diff."));
    let t = words(&doc(&c, &n, &s, &Workbench::default()));
    assert!(!t.contains("PRIVATE PROTOCOL"));
    assert!(t.contains("Now inspect the diff."));
}
#[test]
fn diff_has_an_observed_baseline_and_semantic_colors() {
    let (c, n, s) = fixture();
    let mut u = Workbench::default();
    u.tabs.insert(1, CellTab::Diff);
    let d = doc(&c, &n, &s, &u);
    assert!(words(&d).contains("already applied"));
    // The added and removed lines keep their side's colour, and each one
    // now carries both line numbers, so the row is read by its parts.
    assert!(d.rows.iter().any(|r| {
        r.text.contains("+ new();")
            && r.spans
                .iter()
                .any(|(t, tone)| t.contains("new();") && *tone == Tone::Success)
    }));
    assert!(d.rows.iter().any(|r| {
        r.text.contains("− old();")
            && r.spans
                .iter()
                .any(|(t, tone)| t.contains("old();") && *tone == Tone::Failure)
    }));
}
#[test]
fn absent_diff_is_not_proof_of_no_changes() {
    let (c, mut n, s) = fixture();
    n.cells[0].changes = None;
    let mut u = Workbench::default();
    u.tabs.insert(1, CellTab::Diff);
    assert!(words(&doc(&c, &n, &s, &u)).contains("does not prove no files changed"));
}
#[test]
fn structured_errors_override_fold_and_noise() {
    let (c, mut n, s) = fixture();
    n.cells[0].error = Some(CellError {
        class: "TypeError".into(),
        message: "Missing result".into(),
        ..Default::default()
    });
    let mut u = Workbench::default();
    u.collapsed.insert(1);
    assert!(
        doc(&c, &n, &s, &u)
            .rows
            .iter()
            .any(|r| r.text.contains("Missing result") && r.tone == Tone::Failure)
    );
}
#[test]
fn compiler_chatter_stays_muted() {
    let (c, n, s) = fixture();
    let mut u = Workbench::default();
    u.tabs.insert(1, CellTab::Output);
    assert!(
        doc(&c, &n, &s, &u)
            .rows
            .iter()
            .any(|r| r.text.contains("dependency graph") && r.tone == Tone::Muted)
    );
}
#[test]
fn helper_details_preserve_assignment_model_and_evidence() {
    let (c, n, s) = fixture();
    let mut u = Workbench::default();
    u.helper = Some((1, 0));
    let t = words(&doc(&c, &n, &s, &u));
    for part in [
        "Retain failures",
        "fixture-helper",
        "prepare failure windows",
    ] {
        assert!(t.contains(part), "{t}");
    }
}
#[test]
fn long_wait_has_time_but_no_fake_percentage() {
    let (c, mut n, mut s) = fixture();
    n.cells[0].helpers[0].outcome = HelperOutcome {
        elapsed_ms: 123000,
        ..Default::default()
    };
    s.activity = Activity::Compacting;
    let mut u = Workbench::default();
    u.helper = Some((1, 0));
    let t = words(&doc(&c, &n, &s, &u));
    assert!(t.contains("123.0s"));
    assert!(t.contains("estimate unknown"));
    assert!(!t.contains("Returned:"));
    assert!(!t.contains('%'));
}
#[test]
fn failed_helper_does_not_disappear_in_a_large_roster() {
    let (c, mut n, s) = fixture();
    let h = n.cells[0].helpers[0].clone();
    n.cells[0].helpers = vec![h; 8];
    n.cells[0].helpers[7].outcome.ok = false;
    n.cells[0].helpers[7].outcome.text = "Provider unavailable".into();
    assert!(
        doc(&c, &n, &s, &Workbench::default())
            .rows
            .iter()
            .any(|r| r.text.contains("Provider unavailable") && r.tone == Tone::Failure)
    );
}
#[test]
fn host_lowered_frame_is_labeled_honestly() {
    let (c, mut n, s) = fixture();
    n.cells[0].origin = pane::abi::Origin::DirectTool;
    assert!(words(&doc(&c, &n, &s, &Workbench::default())).contains("Host-lowered"));
}
#[test]
fn tool_stream_is_not_misrepresented_as_executed_source() {
    let (c, n, mut s) = fixture();
    s.streaming_tool_input = Some("{\"code\":\"const".into());
    let d = doc(&c, &n, &s, &Workbench::default());
    assert!(words(&d).contains("not executed"));
    assert!(
        d.rows
            .iter()
            .any(|r| r.text.contains("{\"code") && r.tone == Tone::Muted)
    );
}
#[test]
fn every_theme_and_local_surface_keeps_terminal_background() {
    let (c, n, s) = fixture();
    for theme in Theme::ALL {
        let mut s = s.clone();
        s.theme = theme;
        for mode in 0..4 {
            let mut u = Workbench::default();
            u.work = mode == 1;
            u.approvals = mode == 2;
            u.models = (mode == 3).then(navigator);
            let b = draw(&c, &n, &s, &mut u, 100, 40);
            assert!(b.content.iter().all(|cell| cell.bg == Color::Reset));
        }
    }
}
#[test]
fn resize_cannot_panic_or_leave_click_targets_offscreen() {
    let (c, n, s) = fixture();
    for (w, h) in [
        (1, 1),
        (8, 4),
        (30, 10),
        (60, 22),
        (80, 30),
        (120, 45),
        (200, 55),
    ] {
        for mode in 0..5 {
            let mut u = Workbench::default();
            u.work = mode == 1;
            u.approvals = mode == 2;
            u.models = (mode == 3).then(navigator);
            u.activity = mode == 4;
            draw(&c, &n, &s, &mut u, w, h);
            for (r, _) in &u.geometry.hits {
                assert!(r.right() <= w && r.bottom() <= h, "{w}x{h}: {r:?}");
            }
        }
    }
}
#[test]
fn reduced_motion_freezes_only_decoration() {
    let (c, n, mut s) = fixture();
    s.activity = Activity::Thinking;
    s.reduced_motion = true;
    let mut u = Workbench::default();
    let a = text(&draw(&c, &n, &s, &mut u, 100, 40));
    s.animation_frame = 8;
    assert_eq!(a, text(&draw(&c, &n, &s, &mut u, 100, 40)));
}
#[test]
fn active_animation_changes_at_most_three_cells() {
    let (c, n, mut s) = fixture();
    s.activity = Activity::Thinking;
    let mut u = Workbench::default();
    let a = draw(&c, &n, &s, &mut u, 100, 40);
    s.animation_frame = 8;
    let b = draw(&c, &n, &s, &mut u, 100, 40);
    let changed = a
        .content
        .iter()
        .zip(&b.content)
        .filter(|(a, b)| a != b)
        .count();
    assert!(changed > 0 && changed <= 3, "{changed}");
}
#[test]
fn click_waits_for_release_and_batched_drag_only_copies() {
    let (c, n, mut s) = fixture();
    let mut u = Workbench::default();
    draw(&c, &n, &s, &mut u, 100, 40);
    let (r, _) = u
        .geometry
        .hits
        .iter()
        .find(|(_, a)| *a == Action::Cell(1))
        .unwrap()
        .clone();
    mouse(
        &mut u,
        &mut s,
        &n,
        MouseEventKind::Down(MouseButton::Left),
        r.x,
        r.y,
    );
    assert!(!u.collapsed.contains(&1));
    mouse(
        &mut u,
        &mut s,
        &n,
        MouseEventKind::Drag(MouseButton::Left),
        r.x + 8,
        r.y,
    );
    let e = mouse(
        &mut u,
        &mut s,
        &n,
        MouseEventKind::Up(MouseButton::Left),
        r.x + 8,
        r.y,
    );
    assert!(matches!(e, Effect::Copy(_)), "{e:?}");
    assert!(!u.collapsed.contains(&1));
    click(&mut u, &mut s, &n, Action::Cell(1));
    assert!(u.collapsed.contains(&1));
}
#[test]
fn keyboard_and_mouse_open_the_same_diff() {
    let (c, n, mut s) = fixture();
    let mut a = Workbench::default();
    draw(&c, &n, &s, &mut a, 100, 40);
    click(&mut a, &mut s, &n, Action::Tab(1, CellTab::Diff));
    let mut b = Workbench::default();
    key(&mut b, &mut s, &n, KeyCode::F(4));
    assert_eq!(a.tabs, b.tabs);
}
#[test]
fn scroll_keeps_composer_and_latest_resumes_following() {
    let (mut c, n, mut s) = fixture();
    for _ in 0..40 {
        c.messages
            .push(Message::text(Role::User, "An earlier instruction."));
    }
    let mut u = Workbench::default();
    draw(&c, &n, &s, &mut u, 80, 20);
    let composer = u.geometry.composer;
    mouse(&mut u, &mut s, &n, MouseEventKind::ScrollUp, 10, 5);
    assert!(s.scrollback > 0);
    draw(&c, &n, &s, &mut u, 80, 20);
    assert_eq!(composer, u.geometry.composer);
    click(&mut u, &mut s, &n, Action::Latest);
    assert_eq!(s.scrollback, 0);
}
#[test]
fn modal_navigation_preserves_draft_and_transcript_position() {
    let (c, n, mut s) = fixture();
    s.scrollback = 4;
    s.input = "keep my draft".into();
    let mut u = Workbench::default();
    u.models = Some(navigator());
    draw(&c, &n, &s, &mut u, 100, 40);
    mouse(&mut u, &mut s, &n, MouseEventKind::ScrollDown, 10, 8);
    key(&mut u, &mut s, &n, KeyCode::Char('a'));
    key(&mut u, &mut s, &n, KeyCode::Esc);
    assert_eq!(s.scrollback, 4);
    assert_eq!(s.input, "keep my draft");
}
#[test]
fn unavailable_catalogue_is_hidden_and_cannot_be_selected() {
    let mut m = navigator();
    assert_eq!(m.candidates().len(), 2);
    m.all_sources = true;
    assert_eq!(m.candidates().len(), 3);
    m.query = "unavailable".into();
    m.selected = 0;
    assert!(m.choose().unwrap_err().contains("credential"));
}
#[test]
fn model_search_spaces_do_not_stage_models() {
    let (_, n, mut s) = fixture();
    let mut u = Workbench::default();
    u.models = Some(navigator());
    for c in "A helper".chars() {
        key(&mut u, &mut s, &n, KeyCode::Char(c));
    }
    let m = u.models.as_mut().unwrap();
    assert_eq!(m.query, "A helper");
    m.role = 1;
    assert_eq!(m.choose().unwrap(), "/model helper fixture-helper");
}
#[test]
fn unmeasured_does_not_mean_zero() {
    let mut m = navigator();
    m.scores.insert("fixture-main".into(), 80.0);
    m.measured_order = true;
    let rows = m.candidates();
    assert_eq!(rows[0].score, Some(80.0));
    assert_eq!(rows[1].score, None);
}
#[test]
fn picker_never_offers_implicit_subagent_inheritance() {
    let (c, n, s) = fixture();
    let mut u = Workbench::default();
    let mut m = navigator();
    m.role = 2;
    u.models = Some(m);
    assert!(text(&draw(&c, &n, &s, &mut u, 100, 40)).contains("explicit model required"));
    assert!(!u.geometry.hits.iter().any(
        |(_, a)| matches!(a,Action::Command(c) if c.contains("inherit")||c.ends_with(" auto"))
    ));
}
#[test]
fn settings_browsing_creates_no_file() {
    let (t, _, p) = prefs();
    assert!(!p.path.exists());
    assert!(!t.0.join(".pane/config.toml").exists());
}
#[test]
fn direct_save_and_undo_use_the_native_store() {
    let (_t, mut s, mut p) = prefs();
    p.save("ui.theme", Some("amber".into()), &mut s).unwrap();
    assert_eq!(s.theme, Theme::Amber);
    assert!(std::fs::read_to_string(&p.path).unwrap().contains("amber"));
    p.undo(&mut s).unwrap();
    assert!(p.saved("ui.theme").is_none());
}
#[test]
fn escape_does_not_undo_saved_settings_or_unrelated_session_overrides() {
    let (_t, mut s, mut p) = prefs();
    s.reduced_motion = true;
    p.save("ui.theme", Some("ice".into()), &mut s).unwrap();
    let path = p.path.clone();
    let mut u = Workbench::default();
    u.preferences = Some(p);
    key(&mut u, &mut s, &Notebook::default(), KeyCode::Esc);
    assert!(std::fs::read_to_string(path).unwrap().contains("ice"));
    assert_eq!(s.theme, Theme::Ice);
    assert!(s.reduced_motion);
}
#[test]
fn invalid_setting_leaves_disk_and_live_state_unchanged() {
    let (_t, mut s, mut p) = prefs();
    p.save("ui.theme", Some("ice".into()), &mut s).unwrap();
    let before = std::fs::read(&p.path).unwrap();
    assert!(p.save("ui.theme", Some("invalid".into()), &mut s).is_err());
    assert_eq!(before, std::fs::read(&p.path).unwrap());
    assert_eq!(s.theme, Theme::Ice);
}
#[test]
fn saved_permissions_do_not_change_running_authority() {
    let (_t, mut s, mut p) = prefs();
    let before = s.permissions.rung();
    p.save("permissions.mode", Some("manual".into()), &mut s)
        .unwrap();
    assert_eq!(s.permissions.rung(), before);
    assert!(p.notice.contains("new session"));
}
#[test]
fn concurrent_file_edits_are_not_overwritten() {
    let (_t, mut s, mut p) = prefs();
    p.save("ui.theme", Some("ice".into()), &mut s).unwrap();
    std::fs::write(&p.path, "[ui]\ntheme = \"rose\"\n").unwrap();
    assert!(p.save("ui.theme", Some("amber".into()), &mut s).is_err());
    assert!(std::fs::read_to_string(&p.path).unwrap().contains("rose"));
}
#[test]
fn every_native_key_is_searchable_but_normal_categories_are_bounded() {
    let (_t, _, mut p) = prefs();
    for category in 0..5 {
        p.category = category;
        assert!(p.rows().len() <= 8);
    }
    for spec in pane::settings::specs() {
        p.query = spec.key.into();
        assert!(p.rows().iter().any(|s| s.key == spec.key), "{}", spec.key);
    }
}
#[test]
fn never_ask_requires_confirmation_without_changing_work_or_access() {
    let (c, n, mut s) = fixture();
    let mode = s.mode;
    let mut u = Workbench::default();
    u.approvals = true;
    draw(&c, &n, &s, &mut u, 100, 40);
    click(&mut u, &mut s, &n, Action::Rung("full".into()));
    assert!(u.confirm.is_some());
    assert_ne!(s.permissions.rung(), pane::permissions::Rung::Full);
    key(&mut u, &mut s, &n, KeyCode::Enter);
    assert_eq!(s.permissions.rung(), pane::permissions::Rung::Full);
    assert_eq!(s.mode, mode);
}
#[test]
fn the_live_session_uses_the_new_renderer() {
    let src = include_str!("../src/session/ui.rs");
    let live = src.split("#[cfg(test)]\nmod tests").next().unwrap();
    assert!(live.contains("crate::workbench::render"));
    assert!(!live.contains("tui::render_screen_with_geometry("));
}

#[test]
fn sidebar_visibility_is_respected_without_opaque_surfaces() {
    let (c, n, mut s) = fixture();
    let mut u = Workbench::default();
    s.sidebar = pane::tui::SidebarVisibility::Shown;
    let with = draw(&c, &n, &s, &mut u, 140, 40);
    let shown = u.geometry.transcript.width;
    assert!(text(&with).contains("THIS SESSION"));
    s.sidebar = pane::tui::SidebarVisibility::Hidden;
    let without = draw(&c, &n, &s, &mut u, 140, 40);
    assert!(!text(&without).contains("THIS SESSION"));
    assert!(u.geometry.transcript.width > shown);
}

#[test]
fn reader_anchor_survives_rows_inserted_above_it() {
    let mut u = Workbench::default();
    let mut s = ScreenState {
        scrollback: 10,
        ..Default::default()
    };
    let mut d = Document::default();
    for i in 0..30 {
        d.push(&format!("row {i}"), Tone::Normal, None, 80, i);
    }
    u.anchor = Some((d.rows[12].key, d.rows[12].text.clone()));
    u.last_scrollback = 10;
    d.rows.insert(0, d.rows[0].clone());
    u.anchor_document(&d, &mut s, 8);
    assert_eq!(d.rows[d.rows.len() - 8 - s.scrollback].text, "row 12");
    s.scrollback = 0;
    u.anchor_document(&d, &mut s, 8);
    assert_eq!(s.scrollback, 0);
}

#[test]
fn favorites_picker_assigns_one_slot_and_preserves_other_roles() {
    let mut m = navigator();
    m.role = 2;
    m.slot = Some("quick".into());
    let candidate = m.candidates()[m.selected].model.clone();
    assert_eq!(m.choose().unwrap(), format!("/subagents quick {candidate}"));
    assert!(m.assignment.slots.is_empty());
}

#[test]
fn choosing_a_model_from_global_settings_preserves_scope_and_live_assignment() {
    let (_temp, mut s, mut p) = prefs();
    p.switch_scope().unwrap();
    let path = p.path.clone();
    s.model = Some("live-main".into());
    let mut m = navigator();
    m.role = 1;
    m.target_key = Some("helpers.model".into());
    let chosen = m.candidates()[m.selected].model.clone();
    let mut u = Workbench {
        models: Some(m),
        model_preference: Some((p, "helpers.model".into())),
        ..Default::default()
    };
    let (c, n, _) = fixture();
    draw(&c, &n, &s, &mut u, 110, 40);
    assert!(matches!(
        click(&mut u, &mut s, &n, Action::ChooseModel),
        Effect::Consumed
    ));
    assert_eq!(s.model.as_deref(), Some("live-main"));
    let p = u.preferences.as_ref().unwrap();
    assert_eq!(p.scope, pane::settings::Scope::Global);
    assert!(std::fs::read_to_string(path).unwrap().contains(&chosen));
}

#[test]
fn settings_favorite_removal_and_undo_are_atomic() {
    let (_temp, mut s, mut p) = prefs();
    p.save(
        "agents.slots.quick.model",
        Some("small-model".into()),
        &mut s,
    )
    .unwrap();
    p.save("agents.slots.quick.effort", Some("low".into()), &mut s)
        .unwrap();
    p.save("agents.mode", Some("roster".into()), &mut s)
        .unwrap();
    p.save("agents.slots.quick.model", None, &mut s).unwrap();
    assert!(p.loaded.config.agents.slots.is_empty());
    assert_eq!(p.loaded.config.agents.mode, pane::config::AgentsMode::Off);
    p.undo(&mut s).unwrap();
    assert_eq!(
        p.loaded.config.agents.mode,
        pane::config::AgentsMode::Roster
    );
    assert_eq!(p.loaded.config.agents.slots["quick"].model, "small-model");
}

/// Development aid: prints the rendered workbench so the layout can be read.
/// `cargo test -p pane --test workbench -- --ignored screenshot --nocapture`
#[test]
#[ignore]
fn screenshot() {
    let (c, n, s) = fixture();
    let mut u = Workbench::default();
    for (name, w, h) in [("WIDE 140x40", 140u16, 40u16), ("NARROW 80x30", 80, 30)] {
        let b = draw(&c, &n, &s, &mut u, w, h);
        println!("\n===== {name} =====\n{}", text(&b));
    }
    let (mut c2, n2, mut s2) = fixture();
    s2.activity = Activity::Executing;
    s2.input = "keep the decorative mark still".into();
    c2.messages
        .push(Message::text(Role::User, "and check the tests"));
    c2.messages.push({
        let mut m = Message::text(Role::Assistant, "Checking the guard now.");
        m.content.push(Block::ToolUse{id:"call-2".into(),name:"execute_cell".into(),input:serde_json::json!({"code":"await edit(\"motion.rs\");\nconst t = await checks.run(\"tests\");"})});
        m
    });
    let mut u = Workbench::default();
    let b = draw(&c2, &n2, &s2, &mut u, 140, 40);
    println!("\n===== RUNNING 140x40 =====\n{}", text(&b));
    let mut u = Workbench::default();
    u.tabs.insert(1, CellTab::Diff);
    let b = draw(&c, &n, &s, &mut u, 140, 40);
    println!("\n===== DIFF 140x40 =====\n{}", text(&b));
    let root = Temp::new();
    let mut s3 = s.clone();
    s3.settings_root = Some(root.0.clone());
    let mut u = Workbench::default();
    u.open_settings(&s3);
    let b = draw(&c, &n, &s3, &mut u, 140, 40);
    println!("\n===== SETTINGS 140x40 =====\n{}", text(&b));
}

/// `/diff` opens the last recorded cell on its own diff, which is the route
/// the transcript's "Open diff ↗" and the F4 key also take.
#[test]
fn diff_command_opens_the_last_cell_on_its_diff() {
    let (c, n, mut s) = fixture();
    let mut u = Workbench::default();
    assert!(u.local_command("/diff", &mut s, &n));
    let d = doc(&c, &n, &s, &u);
    assert!(
        d.rows.iter().any(|r| r
            .tabs
            .iter()
            .any(|(label, tab)| label.starts_with("Changes") && *tab == CellTab::Diff)
            && r.action == Some(Action::Tab(1, CellTab::Diff))),
        "{}",
        words(&d)
    );
}

/// A narrowed search leaves nothing of the wider list behind it: the row a
/// filter removed must not be legible anywhere on the surface.
#[test]
fn a_filtered_navigator_leaves_no_row_of_the_wider_list() {
    let (c, n, s) = fixture();
    let mut u = Workbench::default();
    let mut nav = navigator();
    nav.query = "fixture-helper".into();
    u.models = Some(nav);
    let b = draw(&c, &n, &s, &mut u, 80, 30);
    let screen = text(&b);
    assert!(screen.contains("fixture-helper"), "{screen}");
    // The tab row still names the session's main model; the *list* holds
    // one row, and the model the query excluded is not among them.
    let list: String = screen
        .lines()
        .filter(|l| l.contains("subscription") && l.contains('·'))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(list.contains("fixture-helper"), "{screen}");
    assert!(!list.contains("fixture-main"), "{screen}");
    assert!(!screen.contains("unavailable-model"), "{screen}");
}
