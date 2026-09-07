//! Bounded parsing of the assistant's action channel.
//!
//! Several complete `pane` fences form one cell, in message order.  The
//! runtime compiles that one source before executing it, so accepting several
//! fences does not turn them into separately validated cells.

/// A standalone final line the assistant uses to explicitly complete a prose
/// answer.  It is protocol framing and is not displayed to the user.
pub const COMPLETE_MARKER: &str = "<!-- pane:done -->";

/// Keep one response comfortably below the runtime's existing 128 KiB repair
/// bound, even when framing separators are added.
pub const MAX_PROGRAM_BYTES: usize = 128 * 1024;

/// Prevent an adversarial response from making parsing cost depend on an
/// unbounded number of tiny fences.
pub const MAX_PANE_BLOCKS: usize = 32;

/// What one assistant message contained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Extracted {
    /// One or more complete `pane` fences, composed in message order.
    Program(String),
    /// One complete `pane-edit` fence: JSON amending a parse-failed cell.
    Edit(String),
    /// Multiple edits, or an edit mixed with executable source. Neither runs.
    TwoBlocks,
    /// No attempt to use the executable channel.
    Prose,
    /// An attempted executable channel was malformed or exceeded a bound.
    Invalid(String),
}

#[derive(Debug)]
struct Fence<'a> {
    info: &'a str,
    body: String,
    complete: bool,
}

/// Extracts the action channel from an assistant response.
///
/// The fence grammar deliberately stays smaller than general Markdown: three
/// backticks at the start of a line, an exactly matching `pane` or
/// `pane-edit` info string, and a closing line containing exactly three
/// backticks. Other fenced languages are examples and never execute.
pub fn extract_program(assistant_text: &str) -> Extracted {
    let fences = fences(assistant_text);
    let mut programs = Vec::new();
    let mut edits = Vec::new();
    let mut program_bytes = 0usize;

    for fence in fences {
        match fence.info {
            "pane" => {
                if !fence.complete {
                    return Extracted::Invalid("unfinished pane block".into());
                }
                if programs.len() == MAX_PANE_BLOCKS {
                    return Extracted::Invalid(format!(
                        "too many pane blocks; maximum is {MAX_PANE_BLOCKS}"
                    ));
                }
                let separator_bytes = usize::from(!programs.is_empty()) * "\n;\n".len();
                program_bytes = match program_bytes
                    .checked_add(separator_bytes)
                    .and_then(|bytes| bytes.checked_add(fence.body.len()))
                {
                    Some(bytes) if bytes <= MAX_PROGRAM_BYTES => bytes,
                    _ => {
                        return Extracted::Invalid(format!(
                            "pane source exceeds {MAX_PROGRAM_BYTES} bytes"
                        ));
                    }
                };
                programs.push(fence.body);
            }
            "pane-edit" => {
                if !fence.complete {
                    return Extracted::Invalid("unfinished pane-edit block".into());
                }
                if fence.body.len() > MAX_PROGRAM_BYTES {
                    return Extracted::Invalid(format!(
                        "pane-edit exceeds {MAX_PROGRAM_BYTES} bytes"
                    ));
                }
                edits.push(fence.body);
            }
            info if info.starts_with("pane") => {
                return Extracted::Invalid(format!(
                    "malformed executable fence `{info}`; expected `pane` or `pane-edit`"
                ));
            }
            _ => {}
        }
    }

    if malformed_xml_attempt(assistant_text) {
        return Extracted::Invalid("malformed executable pane attempt".into());
    }
    if !edits.is_empty() {
        return if edits.len() == 1 && programs.is_empty() {
            Extracted::Edit(edits.pop().expect("one edit counted above"))
        } else {
            Extracted::TwoBlocks
        };
    }
    match programs.len() {
        0 => Extracted::Prose,
        1 => Extracted::Program(programs.pop().expect("one program counted above")),
        _ => {
            // A statement boundary protects the next block from a trailing
            // line comment and from automatic-semicolon-insertion surprises.
            Extracted::Program(programs.join("\n;\n"))
        }
    }
}

/// Returns a natural prose answer, removing the legacy final marker when one
/// is present. Any attempted executable channel remains outside this path.
pub fn completion_text(text: &str) -> Option<String> {
    if !matches!(extract_program(text), Extracted::Prose) {
        return None;
    }

    if !text.contains(COMPLETE_MARKER) {
        let visible = text.trim_end_matches(['\r', '\n']);
        return (!visible.trim().is_empty()).then(|| visible.to_string());
    }

    let mut in_fence = false;
    let mut marker_start = None;
    let mut offset = 0usize;
    for segment in text.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.starts_with("```") {
            if in_fence {
                if line == "```" {
                    in_fence = false;
                }
            } else {
                in_fence = true;
            }
        } else if !in_fence && line == COMPLETE_MARKER {
            marker_start = Some(offset);
        }
        offset += segment.len();
    }

    let start = marker_start?;
    let tail = &text[start..];
    if tail != COMPLETE_MARKER
        && tail != format!("{COMPLETE_MARKER}\n")
        && tail != format!("{COMPLETE_MARKER}\r\n")
    {
        return None;
    }

    let mut visible = &text[..start];
    if let Some(without_newline) = visible.strip_suffix('\n') {
        visible = without_newline
            .strip_suffix('\r')
            .unwrap_or(without_newline);
    }
    if visible.trim().is_empty() {
        return None;
    }
    Some(visible.to_string())
}

fn fences(text: &str) -> Vec<Fence<'_>> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(rest) = lines[i].strip_prefix("```") else {
            i += 1;
            continue;
        };
        let info = rest.trim();
        let mut body = Vec::new();
        let mut j = i + 1;
        while j < lines.len() && lines[j] != "```" {
            body.push(lines[j]);
            j += 1;
        }
        found.push(Fence {
            info,
            body: body.join("\n"),
            complete: j < lines.len(),
        });
        i = if j < lines.len() { j + 1 } else { lines.len() };
    }
    found
}

fn malformed_xml_attempt(text: &str) -> bool {
    let mut in_fence = false;
    text.lines().any(|line| {
        if line.starts_with("```") {
            if in_fence {
                if line == "```" {
                    in_fence = false;
                }
            } else {
                in_fence = true;
            }
            return false;
        }
        if in_fence {
            return false;
        }
        let line = line.trim_start().to_ascii_lowercase();
        line.starts_with("<pane")
            || line.starts_with("</pane")
            || (line.starts_with('<') && line.contains("-pane"))
    })
}
