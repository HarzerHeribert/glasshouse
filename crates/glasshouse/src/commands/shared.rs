//! `commands::shared` -- moved verbatim from `main.rs` (Phase 59 decomposition).

use glasshouse::session::{SessionDisposition, SessionRecord};

/// An objective as one table cell: first line only, and bounded.
///
/// A checkpoint's objective is free text a person wrote and may well be a
/// paragraph. A listing that let one row become forty would be unreadable, so
/// the table shows the first line and `checkpoint show` prints the rest.
pub(crate) fn one_line(text: &str) -> String {
    const WIDTH: usize = 60;
    let first = text.lines().next().unwrap_or("").trim();
    if first.chars().count() <= WIDTH {
        return first.to_owned();
    }
    // By characters, never by bytes: cutting a multi-byte character in half
    // would put invalid text on a terminal.
    let cut: String = first.chars().take(WIDTH - 1).collect();
    format!("{cut}…")
}

/// Enough of an identifier to name a session in conversation.
///
/// The full identifier stays available in `--log-level` output and is what any
/// command taking a session takes; this is only for the eye.
pub(crate) fn short_id(id: &glasshouse::session::SessionId) -> String {
    id.as_str().chars().take(12).collect()
}

/// Which of the four categories a session list has to separate.
///
/// One function, used by both the listing and the detail view, so the two can
/// never disagree about whether a session is resumable.
pub(crate) fn disposition_word(record: &SessionRecord) -> &'static str {
    match record.disposition() {
        SessionDisposition::Active => "active",
        SessionDisposition::Resumable => "resumable",
        SessionDisposition::Closed => "closed",
        SessionDisposition::Failed => "failed",
    }
}

/// A rough "how long ago", which is what a session list is actually read for.
pub(crate) fn format_age(timestamp: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    // A timestamp in the future is possible — a clock corrected backwards
    // between writing the row and reading it — and produces a negative value
    // here, because `saturating_sub` saturates at `i64::MIN`, not at zero. The
    // first arm covers it: reporting "just now" is the honest answer, and it
    // avoids printing a confident negative age. An explicit `< 0` guard used
    // to sit here returning the same string, which only obscured that.
    let seconds = now.saturating_sub(timestamp);
    match seconds {
        s if s < 60 => "just now".to_owned(),
        s if s < 3_600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3_600),
        s => format!("{}d ago", s / 86_400),
    }
}
