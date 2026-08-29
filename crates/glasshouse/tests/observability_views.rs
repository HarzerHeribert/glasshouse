//! Phase 47's diagnostic views, exercised the way a caller outside this crate
//! reaches them: through the `pub` shell state and the `pub` renderer, drawn
//! into a real Ratatui backend, with no access to anything this crate keeps
//! `pub(crate)`.
//!
//! Behavioral contract, for map line 1765: route health, immediate
//! availability, cadence, quota reset and failure-domain evidence each reach
//! the screen as their own labelled concept, never folded into one status
//! word; a value no provider stated reads `unknown` rather than a zero; and
//! the view never claims two resources are in independent failure domains,
//! because nothing in this build can establish that.
//!
//! What only an external suite can catch is a mistake in *what this crate
//! actually exports* — `RouteHealthRow`, `RouteHealthState::rows`,
//! `ShellState::open_route_health` and `Overlay::RouteHealth` all have to be
//! reachable from outside for this view to be testable at all, and the
//! in-crate `shell::view::tests` module cannot notice if one of them is
//! quietly `pub(crate)`.
//!
//! Deliberately **not** a re-proof of `shell::build_route_health_table`: that
//! function is private to the crate, and the caches it reads are written by
//! the gateway's accept loop, which `gateway::conformance` already covers.
//! This suite is about what a user sees.
//!
//! # What this suite structurally cannot reach, and what does
//!
//! Everything here calls `ShellState::open_route_health` itself, with rows it
//! built itself. That is the right shape for a question about rendering, and
//! it is also why deleting the run loop's own
//! `state.open_route_health(build_route_health_table(runtime))` argument
//! changed nothing in this file — the mutation `phase-47.md` records as
//! SURVIVED against 275 tests, and practice §35's "a caller every test
//! bypasses is not a caller" one layer out.
//!
//! `tests/tui_harness.rs` is what closes that: it starts the shipped binary on
//! a real pty, presses `h`, and reads the rendered screen back through a
//! terminal emulator, so the run loop's dispatch arm is on the path between
//! the cache on disk and the assertion. The two suites are complementary and
//! neither replaces the other — this one is fast, exercises the crate's public
//! surface, and can vary a row field by field; that one is the only thing that
//! fails when the key stops reaching the view.

use glasshouse::shell::state::{Overlay, RouteHealthRow, ShellState};
use glasshouse::shell::view::render;

use ratatui::Terminal;
use ratatui::backend::TestBackend;

/// Render the shell and flatten the buffer to text, so an assertion is about
/// what a user would actually see.
fn rendered(state: &ShellState, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render(state, frame))
        .expect("draw must not panic");
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The same screen with the popup's border columns dropped and whitespace
/// collapsed, so a phrase the renderer's own word-wrap split across two rows
/// can still be matched. The per-line assertions below use the raw text
/// instead — "not on the same line" is a claim about lines.
fn flattened(text: &str) -> String {
    text.replace('│', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell() -> ShellState {
    ShellState::new("glasshouse", "/tmp/project", "0.1.0", Vec::new())
}

/// A resource whose five concepts disagree: it has never failed, yet it is
/// unavailable because the provider refused the credential; Glasshouse is
/// pacing it; and the provider's own window resets at a different time.
fn conflicted_row(now: i64) -> RouteHealthRow {
    RouteHealthRow {
        provider: "anyrouter".to_owned(),
        credential_label: "anyrouter/ANYROUTER_API_KEY".to_owned(),
        model: "claude-opus-4-1".to_owned(),
        consecutive_failures: 0,
        credential_rejected: true,
        available_now: false,
        cooling_down_until_unix: Some(now + 600),
        stated_limit: Some(300),
        stated_window_seconds: Some(60),
        quota_resets_at_unix: Some(now + 3_600),
        failure_domain: "shared".to_owned(),
        failure_domain_peers: 2,
    }
}

/// Map line 1765, from outside the crate: five concepts, five labels, and no
/// line carrying two of them.
///
/// Rendered at a realistic width and a wide one — practice §17 — because a
/// label that clipped off-screen would satisfy a `contains` for the wrong
/// reason at one width and fail at the other.
#[test]
fn the_route_health_view_keeps_five_concepts_separate_at_two_widths() {
    const CONCEPTS: [&str; 5] = [
        "route health",
        "immediate availability",
        "cadence",
        "quota reset",
        "failure domain",
    ];
    let now = glasshouse::provider::cache::now_unix_seconds();

    for (width, height) in [(120, 40), (400, 40)] {
        let mut state = shell();
        state.open_route_health(vec![conflicted_row(now)]);
        assert_eq!(state.overlay(), Some(Overlay::RouteHealth));

        let text = rendered(&state, width, height);
        for concept in CONCEPTS {
            assert!(
                text.contains(concept),
                "line 1765 names `{concept}`; it must be its own label, \
                 width {width}:\n{text}"
            );
        }
        for line in text.lines() {
            let folded = CONCEPTS
                .iter()
                .filter(|concept| line.contains(*concept))
                .count();
            assert!(
                folded <= 1,
                "two of line 1765's concepts were folded onto one line, \
                 width {width}:\n{line}"
            );
        }

        // The five disagree, which is why one status word cannot carry them.
        let flat = flattened(&text);
        assert!(
            flat.contains("0 consecutive failure(s)"),
            "a resource with no failures must say so, width {width}:\n{text}"
        );
        assert!(
            flat.contains("credential rejected: yes"),
            "a refused credential is its own health fact, width {width}:\n{text}"
        );
        assert!(
            flat.contains("not schedulable right now"),
            "availability is its own answer, width {width}:\n{text}"
        );
        assert!(
            flat.contains("300 request(s) per 60s"),
            "the provider's stated cadence must reach the screen, \
             width {width}:\n{text}"
        );
    }
}

/// The honesty half of the phase: what no provider stated reads `unknown`,
/// never a zero and never an invented time.
#[test]
fn what_no_provider_stated_reads_unknown_rather_than_zero() {
    let mut state = shell();
    state.open_route_health(vec![RouteHealthRow {
        provider: "openrouter".to_owned(),
        credential_label: "openrouter/OPENROUTER_API_KEY".to_owned(),
        model: "some-free-model".to_owned(),
        consecutive_failures: 0,
        credential_rejected: false,
        available_now: true,
        cooling_down_until_unix: None,
        stated_limit: None,
        stated_window_seconds: None,
        quota_resets_at_unix: None,
        failure_domain: "unknown".to_owned(),
        failure_domain_peers: 0,
    }]);

    let flat = flattened(&rendered(&state, 200, 30));
    assert!(
        flat.contains("quota reset unknown — no response has stated one"),
        "an unstated reset must read `unknown`:\n{flat}"
    );
    assert!(
        flat.contains("provider stated: unknown"),
        "an unstated cadence must read `unknown`:\n{flat}"
    );
    for invented in ["quota reset in", "per 0s", "0 request(s)"] {
        assert!(
            !flat.contains(invented),
            "an unstated value was rendered as `{invented}`:\n{flat}"
        );
    }
}

/// `glasshouse::routing::domain::FailureDomain::Independent` is a state this
/// build cannot earn, so the view must never assert it — asserted at a wide
/// viewport too, because an absence claim is only as strong as the screen it
/// renders into (practice §17).
#[test]
fn the_view_never_claims_independent_failure_domains() {
    let now = glasshouse::provider::cache::now_unix_seconds();
    for (width, height) in [(120, 40), (400, 40)] {
        let mut state = shell();
        state.open_route_health(vec![conflicted_row(now)]);
        let flat = flattened(&rendered(&state, width, height));
        assert_eq!(
            flat.matches("independent").count(),
            flat.matches("never `independent`").count(),
            "`independent` may appear only inside the sentence refusing it, \
             width {width}:\n{flat}"
        );
    }
}

/// Two isolation facts, both asserted rather than assumed.
///
/// The gateway telemetry caches this view is fed from are keyed by provider
/// under the installation's data directory, so the view must label its own
/// scope; and nothing on the screen may carry a credential, a token, a base
/// URL or any project content — `CredentialId::label` is two names, and this
/// is the screen that would leak it if that ever stopped being true.
#[test]
fn the_route_health_view_names_its_scope_and_prints_no_secret() {
    let now = glasshouse::provider::cache::now_unix_seconds();
    let mut state = shell();
    state.open_route_health(vec![conflicted_row(now)]);
    let text = rendered(&state, 200, 40);

    assert!(
        text.contains("not scoped to this project"),
        "the view must say that these readings are installation-wide:\n{text}"
    );

    // A credential label is `provider/VARIABLE`; a *value* would look like
    // none of these, and none of them may ever appear.
    for forbidden in ["sk-", "Bearer", "http://", "https://", "Authorization"] {
        assert!(
            !text.contains(forbidden),
            "the route-health view must never print `{forbidden}`:\n{text}"
        );
    }
}

/// Map line 1770 for this surface: a diagnostic view stays optional. It is
/// absent from the default screen and reached only by pressing its own key.
#[test]
fn the_route_health_view_is_absent_until_its_key_is_pressed() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let state = shell();
    for (width, height) in [(100, 24), (400, 24)] {
        let text = rendered(&state, width, height);
        assert!(
            !text.contains("immediate availability"),
            "the default screen must not show the route-health view, \
             width {width}:\n{text}"
        );
    }

    let mut state = shell();
    assert_eq!(
        state.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)),
        glasshouse::shell::Action::OpenRouteHealth,
        "`h` is the key that asks for this view"
    );
    state.open_route_health(Vec::new());
    assert_eq!(state.overlay(), Some(Overlay::RouteHealth));
    assert!(
        rendered(&state, 120, 24).contains("no gateway exchange has been observed"),
        "an installation with nothing observed must say so rather than \
         showing a table of zeroes"
    );
}
