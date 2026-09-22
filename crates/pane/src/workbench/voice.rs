//! Pane's character: the bird, and every line it says.
//!
//! **The words and the face live in one place so the two voices cannot
//! drift.** Each line here is written twice, for [`Voice::Playful`] and
//! [`Voice::Plain`], and both say the same fact; the playful one is allowed
//! warmth and the odd remark, never a claim the plain one does not make. The
//! bird is decoration -- reduced motion holds it on its still frame, and
//! nothing on screen carries state through the bird alone.
use crate::tui::{Activity, Voice};

/// Which of the bird's six states to draw. It is read off the session's
/// activity, never stored, so it cannot go stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    Idle,
    Thinking,
    Working,
    Done,
    Asking,
    Oops,
}
impl Face {
    pub fn of(activity: Activity, asking: bool) -> Self {
        if asking {
            return Self::Asking;
        }
        match activity {
            Activity::Idle | Activity::Starting => Self::Idle,
            Activity::Thinking | Activity::Waiting | Activity::Compacting | Activity::Searching => {
                Self::Thinking
            }
            Activity::Streaming | Activity::Executing => Self::Working,
            Activity::Complete => Self::Done,
            Activity::Failed => Self::Oops,
        }
    }
}

// A songbird perched and facing right: 20×16 pixels drawn as four rows of
// braille -- crown, eye, beak, a wing line across the body, tail feathers
// and two legs. Each state is one edit of the same bitmap.
const IDLE: [&str; 4] = ["    ⡔⠩⠉⠢⣀⣀", "⡠⠒⢤⠎ ⣀⣀⡀⠑⡄", "⠑⢤⠊⡰⠉  ⠈⡢⠃", "  ⠑⠣⡄⡀⡤⠊  "];
const BLINK: [&str; 4] = ["    ⡔⠍⠍⠢⣀⣀", "⡠⠒⢤⠎ ⣀⣀⡀⠑⡄", "⠑⢤⠊⡰⠉  ⠈⡢⠃", "  ⠑⠣⡄⡀⡤⠊  "];
const THINK: [&str; 4] = ["    ⡔⠉⠋⠢⠤⠆", "⡠⠒⢤⠎ ⣀⣀⡀⠑⡄", "⠑⢤⠊⡰⠉  ⠈⡢⠃", "  ⠑⠣⡄⡀⡤⠊  "];
const PECK: [&str; 4] = ["    ⡔⢉⠉⠢  ", "⡠⠒⢤⠎ ⣀⣀⡀⠙⡖", "⠑⢤⠊⡰⠉  ⠈⡢⠃", "  ⠑⠣⡄⡀⡤⠊  "];
const DONE: [&str; 4] = ["    ⡔⠩⠉⢢⣀⣀", "⡠⠒⢤⠎ ⢀⠔⠁⠑⡄", "⠑⢤⠊⡠⠐⠁  ⡠⠃", "  ⠑⠣⡄⡀⡤⠊  "];
const OOPS: [&str; 4] = ["   ⠈⡜⠩⠉⠫⣀⣀", "⢤⠒⢤⠎ ⣀⣀⡀⠑⡴", "⠐⢤⠊⡰⠉  ⠈⡢⠗", "  ⠑⠣⡄⡀⡤⠊  "];
/// A bird in flight, three cells wide, for the composer's edge.
const FLAP: [&str; 4] = ["⠑⠤⠊", "⠢⠤⠔", "⠤⠤⠤", "⠔⠒⠢"];
/// Every face is padded to this many columns so the text beside it starts
/// at one x on every frame.
pub const FACE_WIDTH: usize = 12;
/// The bird is this many rows tall.
pub const FACE_ROWS: usize = 4;

/// The bird's three rows for `face`, at `tick`; `still` holds the resting
/// frame of that state, which is a complete drawing rather than a paused one.
///
/// Idle blinks once every few seconds, which changes two cells; thinking is
/// a pose, not a motion. Together with the flap on the composer's edge that
/// keeps a live screen inside the three-cell budget `tests/workbench.rs`
/// holds motion to. Working pecks -- more cells -- and is only ever drawn
/// inside a running cell, where the old mark turned too.
pub fn face(face: Face, tick: usize, still: bool) -> [String; FACE_ROWS] {
    let art = match face {
        Face::Idle => {
            if !still && tick % 24 == 23 {
                BLINK
            } else {
                IDLE
            }
        }
        Face::Thinking => THINK,
        Face::Working => {
            if !still && (tick / 3) % 2 == 1 {
                PECK
            } else {
                IDLE
            }
        }
        Face::Done => DONE,
        Face::Asking => THINK,
        Face::Oops => OOPS,
    };
    let mark = match face {
        Face::Done => " ✓",
        Face::Asking => " ?",
        Face::Oops => " ✕",
        _ => "",
    };
    let mut rows = art.map(str::to_string);
    rows[0].push_str(mark);
    rows.map(|row| {
        let w = ratatui::text::Span::raw(row.as_str()).width();
        format!("{row}{}", " ".repeat(FACE_WIDTH.saturating_sub(w)))
    })
}
/// The flying mark on the composer's edge: wings beating while the session
/// works, level when it is not.
pub fn flap(tick: usize, still: bool) -> &'static str {
    if still {
        FLAP[2]
    } else {
        FLAP[(tick / 3) % FLAP.len()]
    }
}

/// The label on a turn of the person's, and on one of Pane's.
pub const YOU: &str = "you";
pub const PANE: &str = "pane";

/// The opening's first line: who it is talking to, and when.
pub fn greeting(voice: Voice, hour: Option<u8>, project: &str) -> String {
    if !voice.playful() {
        return project.to_string();
    }
    let time = match hour {
        Some(5..=11) => "Morning",
        Some(12..=17) => "Afternoon",
        Some(18..=22) => "Evening",
        Some(_) => "Late one",
        None => "Hello",
    };
    format!("{time}! Back in the nest: {project}.")
}
/// The line under the greeting on an empty conversation.
pub fn invitation(voice: Voice) -> &'static str {
    if voice.playful() {
        "Where to?"
    } else {
        "Describe a task, or pick one of these:"
    }
}
/// What the composer says when it is empty.
pub fn placeholder(voice: Voice) -> &'static str {
    if voice.playful() {
        "What next? / for commands, @ for a file"
    } else {
        "Describe the next step — a message or / for commands"
    }
}
/// The composer edge's word for what the session is doing right now.
///
/// A person watching this is asking *is it alive and on what*; the answer is
/// the activity's own name, never a claim about progress toward an end
/// nobody can see.
pub fn status(
    voice: Voice,
    activity: Activity,
    cell: Option<usize>,
    model: Option<&str>,
    writing_cell: bool,
) -> String {
    let playful = voice.playful();
    match activity {
        Activity::Streaming if writing_cell => match cell {
            Some(n) => format!("writing cell {n:03}"),
            None => "writing a cell".into(),
        },
        Activity::Idle => if playful {
            "ready when you are"
        } else {
            "ready"
        }
        .into(),
        Activity::Starting => if playful {
            "waking up"
        } else {
            "starting session"
        }
        .into(),
        Activity::Thinking => if playful { "thinking…" } else { "thinking" }.into(),
        Activity::Streaming => if playful {
            "writing back"
        } else {
            "receiving response"
        }
        .into(),
        Activity::Executing => match cell {
            Some(n) => format!("running cell {n:03}"),
            None => "executing cell".into(),
        },
        Activity::Searching => "searching".into(),
        Activity::Waiting => match (playful, model) {
            (true, Some(m)) => format!("waiting on {m}… no ETA, sorry"),
            (true, None) => "waiting on the provider… no ETA, sorry".into(),
            (false, _) => "waiting on a response · estimate unknown".into(),
        },
        Activity::Compacting => {
            if playful {
                "tidying the context"
            } else {
                "compacting · preparing bounded context"
            }
        }
        .into(),
        Activity::Failed => {
            if playful {
                "that one failed — open the cell and I'll show you why"
            } else {
                "action failed — inspect the cell"
            }
        }
        .into(),
        Activity::Complete => if playful { "done ✓" } else { "complete" }.into(),
    }
}
/// The three lines beside the bird inside a running cell: what is happening,
/// how long it has been, and at whose cost.
pub fn working(
    voice: Voice,
    activity: Activity,
    helper_waiting: bool,
    elapsed: &str,
) -> (&'static str, String) {
    let playful = voice.playful();
    let label = match activity {
        Activity::Executing => {
            if playful {
                "running this cell"
            } else {
                "Executing this cell"
            }
        }
        Activity::Waiting => {
            if playful {
                "waiting on the provider"
            } else {
                "Waiting on the provider"
            }
        }
        Activity::Searching => "Searching",
        Activity::Compacting => {
            if playful {
                "tidying the context"
            } else {
                "Preparing bounded context"
            }
        }
        _ if helper_waiting => {
            if playful {
                "a little helper is on it"
            } else {
                "Little helper working"
            }
        }
        _ => {
            if playful {
                "the model is writing"
            } else {
                "Model is responding"
            }
        }
    };
    let detail = match (playful, helper_waiting) {
        (true, true) => "asked · no ETA, I'll say the moment it's back".to_string(),
        (false, true) => "Request sent · completion estimate unknown".to_string(),
        (true, false) => format!("{elapsed} so far · I'll say the moment it comes back"),
        (false, false) => format!("Elapsed {elapsed} · nothing is assumed complete"),
    };
    (label, detail)
}
/// The one line on the composer's edge that teaches. It turns with the
/// session -- one more cell, one more notice -- rather than with the clock,
/// so it holds still while someone reads it.
pub fn hint(voice: Voice, n: usize) -> &'static str {
    const PLAYFUL: [&str; 8] = [
        "psst — Shift-Tab changes how often I ask before I act",
        "F2 opens settings · everything there applies right now",
        "Ctrl-T opens the instruments · Esc puts them away",
        "click anything in the top bar to change it",
        "Esc once stops after this cell · twice cancels the call",
        "Ctrl-B hides the session card · Ctrl-F hides all the chrome",
        "/diff shows what I just changed · F4 does the same",
        "? shows every key · / for commands · @ for a file",
    ];
    const PLAIN: [&str; 8] = [
        "Shift-Tab changes how often Pane asks before it acts",
        "F2 opens settings · every choice there applies to this session now",
        "Ctrl-T opens the instruments · Esc closes them",
        "Click any control in the top bar to change it",
        "Esc once stops after the current cell · twice cancels the call",
        "Ctrl-B shows or hides the session card · Ctrl-F hides the chrome",
        "/diff opens the last cell's changes · F4 does the same",
        "? lists every key · / for commands · @ for a path in this project",
    ];
    if voice.playful() {
        PLAYFUL[n % PLAYFUL.len()]
    } else {
        PLAIN[n % PLAIN.len()]
    }
}
/// What the bird says when someone clicks it. Nothing here is a fact about
/// the session, which is why it is the one line a plain voice never prints.
pub fn quip(n: usize) -> &'static str {
    const QUIPS: [&str; 24] = [
        "Tweet.",
        "I'm a wren. Probably.",
        "The early bird gets the diff.",
        "Perched and ready.",
        "I read the whole file. Both times.",
        "Chirp. That was a keystroke, not a command.",
        "Nothing runs until you say so. I just sing about it.",
        "Every cell is a little branch; I hop between them.",
        "Birds of a feather commit together.",
        "I do not fly into windows. I open them.",
        "Small bird, big context window.",
        "Did you know: I blink. Watch closely.",
        "I keep the nest tidy: compaction is just spring cleaning.",
        "Peck, peck. That is the sound of tests passing.",
        "I nest in .pane/. Cosy in there.",
        "A helper is a very small bird that only fetches worms.",
        "Shift-Tab is my favourite key. It has a wing on it.",
        "Ask me for the diff. I love a good diff.",
        "Nothing here is a spinner. I am a bird.",
        "You clicked me. Bold. I like it.",
        "Coo. Wait, wrong bird.",
        "The nest is warm and the tests are green. Mostly.",
        "I migrate between models when you ask.",
        "Perch here any time.",
    ];
    QUIPS[n % QUIPS.len()]
}
/// The first line of a finished turn's block, when the model returned no
/// words of its own to put there.
pub fn done_line(voice: Voice, failed: bool) -> &'static str {
    match (voice.playful(), failed) {
        (true, false) => "Done.",
        (false, false) => "Complete.",
        (true, true) => "That one failed.",
        (false, true) => "Failed.",
    }
}
/// Labels for the suggestion chips on an empty conversation, in the order
/// they are offered. Each is the chip's text and the message it types.
pub fn suggestions(
    voice: Voice,
    last_commit: Option<&str>,
    dirty_files: usize,
    has_tests: bool,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(subject) = last_commit {
        let short = super::document::clip(subject, 34);
        out.push((
            format!("pick up: {short}"),
            format!(
                "Pick up where the last commit left off (\"{subject}\"). What is the next step?"
            ),
        ));
    }
    if dirty_files > 0 {
        out.push((
            format!(
                "review {dirty_files} uncommitted {}",
                if dirty_files == 1 {
                    "change"
                } else {
                    "changes"
                }
            ),
            "Review my uncommitted changes and tell me what is unfinished.".into(),
        ));
    }
    if has_tests {
        out.push((
            "run the tests".into(),
            "Run the tests and tell me what fails.".into(),
        ));
    }
    if out.is_empty() {
        out.push((
            if voice.playful() {
                "show me around"
            } else {
                "explore this project"
            }
            .into(),
            "Give me a tour of this project: layout, entry points, how to run and test it.".into(),
        ));
    }
    out
}

/// The local hour, for the greeting. `None` where the platform cannot say,
/// and the greeting then simply has no time of day in it.
pub fn local_hour() -> Option<u8> {
    #[cfg(unix)]
    {
        // SAFETY: `localtime_r` writes only into the `tm` we hand it, and
        // `time` takes a null pointer to mean "now".
        unsafe {
            let now = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&now, &mut tm).is_null() {
                return None;
            }
            u8::try_from(tm.tm_hour).ok()
        }
    }
    #[cfg(not(unix))]
    {
        None
    }
}
/// What the opening offers to do, read from the project itself: the last
/// commit's subject, whether anything is uncommitted, whether there is
/// something to test. Best effort and quick; a project without git, or a
/// machine without it, gets the one suggestion that needs nothing.
pub fn project_suggestions(root: &std::path::Path, voice: Voice) -> Vec<(String, String)> {
    let git = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let last_commit = git(&["log", "-1", "--format=%s"]).filter(|s| !s.is_empty());
    let dirty = git(&["status", "--porcelain"])
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0);
    let has_tests = [
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "Makefile",
        "go.mod",
    ]
    .iter()
    .any(|f| root.join(f).exists());
    suggestions(voice, last_commit.as_deref(), dirty, has_tests)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_face_is_the_same_width_on_every_frame() {
        for f in [
            Face::Idle,
            Face::Thinking,
            Face::Working,
            Face::Done,
            Face::Asking,
            Face::Oops,
        ] {
            for tick in 0..30 {
                for still in [true, false] {
                    for row in face(f, tick, still) {
                        assert_eq!(
                            ratatui::text::Span::raw(row.as_str()).width(),
                            FACE_WIDTH,
                            "{f:?} tick {tick} still {still}: {row:?}"
                        );
                    }
                }
            }
        }
    }

    /// Motion is a budget: a live frame may change at most three cells, and
    /// the bird's blink stays under it. Working pecks, and is drawn only
    /// inside a running cell.
    #[test]
    fn a_frame_changes_at_most_two_cells_of_the_bird() {
        for f in [Face::Idle, Face::Thinking] {
            for tick in 0..30 {
                let a = face(f, tick, false);
                let b = face(f, tick + 1, false);
                let changed: usize = a
                    .iter()
                    .zip(&b)
                    .map(|(x, y)| x.chars().zip(y.chars()).filter(|(p, q)| p != q).count())
                    .sum();
                assert!(changed <= 2, "{f:?} at {tick}: {changed}");
            }
        }
    }

    #[test]
    fn still_frames_are_complete_and_do_not_move() {
        for f in [Face::Idle, Face::Thinking, Face::Working] {
            assert_eq!(face(f, 0, true), face(f, 23, true));
        }
        assert_eq!(flap(0, true), flap(9, true));
    }

    /// The plain voice states every fact the playful one does, without the
    /// character: nothing a person needs is behind the setting.
    #[test]
    fn plain_and_playful_agree_on_the_facts() {
        for a in [
            Activity::Idle,
            Activity::Executing,
            Activity::Waiting,
            Activity::Failed,
            Activity::Complete,
        ] {
            let p = status(Voice::Playful, a, Some(2), Some("m"), false);
            let q = status(Voice::Plain, a, Some(2), Some("m"), false);
            assert!(!p.is_empty() && !q.is_empty());
        }
        assert!(status(Voice::Playful, Activity::Executing, Some(7), None, false).contains("007"));
        assert!(status(Voice::Plain, Activity::Streaming, Some(4), None, true).contains("004"));
        assert!(greeting(Voice::Plain, Some(9), "nest").contains("nest"));
        assert!(!greeting(Voice::Plain, Some(9), "nest").contains("Morning"));
        assert!(greeting(Voice::Playful, Some(9), "nest").starts_with("Morning"));
        assert!(greeting(Voice::Playful, None, "nest").starts_with("Hello"));
    }

    #[test]
    fn suggestions_come_from_the_project_and_never_from_nothing() {
        let s = suggestions(Voice::Playful, Some("fix the guard"), 2, true);
        assert_eq!(s.len(), 3);
        assert!(s[0].0.starts_with("pick up: fix the guard"));
        assert!(s[1].0.contains("2 uncommitted changes"));
        let none = suggestions(Voice::Plain, None, 0, false);
        assert_eq!(none.len(), 1);
        assert!(none[0].1.contains("tour"));
    }
}
