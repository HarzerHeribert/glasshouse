//! The bytes the model receives and the one thing it sends back —
//! `docs/product/pane/model-contract.md`. This module renders the system
//! block and each turn's result message, and extracts the model's program
//! from its reply. It runs nothing and calls nothing outside itself: every
//! function here is a pure string transformation over its own plain input
//! types.
//!
//! **No message this module builds carries a second serialisation of a call
//! and its outcome.** A handle's rendered text arrives already made, in
//! [`CellResult::handle_table`]; this module has no type standing in for a
//! provider-native call-and-answer pair and never produces one.

pub mod declarations;

use crate::contract::{Block, Conversation};
use crate::runtime::outcome::PlanItem;
use crate::tools::registry::{Arg, Tool};

/// `model-contract.md` §2, verbatim. Compared byte for byte by
/// `prompt_bytes.rs::the_preamble_is_the_contracts_verbatim`.
pub const PREAMBLE: &str = "You are Pane, a coding assistant. Answer conversational questions directly\nin prose. To act with tools, write TypeScript in exactly one fenced `pane`\nblock:\n\n```pane\nconst file = await read({path: \"example.txt\"});\nconsole.log(file.text);\n```\n\nUse triple backticks, not XML tags. Only `pane` code executes; a syntax\nerror may offer `pane-edit` to amend it.\nTool results are live objects. Use their declared fields in code; the\nhandle table shows bounded previews, not full payloads.\n\nTop-level bindings persist between cells of the same user request only;\nredeclaring replaces them. A new user request starts a fresh runtime.\nEarlier requests are history, not unfinished work. Answer the current\nrequest; a prose answer ends the request without running tools.\nRunning off the end yields results and another turn. `yieldNow(reason)`\nalso yields. A top-level `return` ends the task; return a string to answer\nthe person, grounded in results you actually observed.\nTo interpret file contents, read and yield first, then answer from the\nnext turn's preview. You may return values computed directly from objects.\n\nA thrown error comes back with its source position and completed bindings.\nContinue from that state; failed or skipped calls did not succeed.\nPermissionDenied is final: code cannot widen the session's sandbox grant.";

/// Request-only context; keeps user text and saved conversation unchanged.
/// A new runtime is created per user request, not per inference turn.
pub fn with_task_context(conversation: &Conversation, model: &str, task: &str) -> Conversation {
    let mut request = conversation.clone();
    request.system.push_str(&format!(
        "\n\nYou are Pane, a coding assistant. Configured request model: {}. This is the requested model, not independently verified backend identity. Do not infer a different identity from previous replies or project paths.",
        serde_json::to_string(model).expect("model name serializes")
    ));
    if let Some(message) = request.messages.iter_mut().rev().find(|message| {
        message.role == crate::contract::Role::User
            && message.content.len() == 1
            && matches!(&message.content[0], Block::Text(text) if text == task)
    }) {
        message.content.push(Block::Text(
            "[Pane task boundary: this is the current user request. Its runtime started empty; variables, handles and jobs from earlier completed requests are not live. Bindings created while answering THIS request persist between its cells. A cell error does not reset completed bindings.]".into()
        ));
    }
    request
}

/// Why the preamble is being replaced: §6's spent task budget, or three
/// prose turns in a row (the primary's addendum of 2026-09-06 — a model that
/// never programs must not spend hundreds of requests to find out).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExhaustedReason {
    TaskBudget,
    ThreeTurnsWithoutAProgram,
}

/// The one sentence that replaces the preamble when the task is exhausted —
/// §6's last paragraph, naming the reason. The only permitted action is a
/// top-level `return`.
pub fn exhausted_preamble(reason: ExhaustedReason) -> &'static str {
    match reason {
        ExhaustedReason::TaskBudget => {
            "The task budget is exhausted; the only action this turn may take is a top-level \
             `return`."
        }
        ExhaustedReason::ThreeTurnsWithoutAProgram => {
            "Three turns without a program; the only action this turn may take is a top-level \
             `return`."
        }
    }
}

/// What this particular session is, as the model needs to know it.
///
/// **The invariant: every field here is a fact the model cannot derive from
/// the preamble and would otherwise discover by failing.** A model that does
/// not know the tool set has no writer spends its turns asking `bash` to do
/// it and reading `PermissionDenied`; a model that does not know which
/// commands are admitted cannot tell a refusal it caused from one the person
/// configured. Observed 2026-09-06: a session spent six cells rediscovering
/// exactly these two facts and then returned a stub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFacts {
    /// The project root every relative path resolves against.
    pub root: String,
    /// `Write`/`Edit` globs the compiled profile actually holds.
    pub writable: Vec<String>,
    /// How many `Bash(...)` patterns are admitted, ignored when
    /// [`SessionFacts::all_commands`] is set.
    pub command_patterns: usize,
    /// Every command line is admitted — a bare `Bash` grant, which is what
    /// `--yolo` synthesises.
    pub all_commands: bool,
    /// Whether the sandbox grants network access.
    pub network: bool,
}

/// §1's *This session* block: the facts above, rendered.
///
/// Pure in its argument, so the golden test that pins the binary's system
/// bytes can build the same string without running a session.
pub fn render_session_facts(facts: &SessionFacts) -> String {
    let writable = if facts.writable.is_empty() {
        "nothing is writable".to_string()
    } else {
        format!(
            "write allow rules: {} (deny rules still apply)",
            facts.writable.join(", ")
        )
    };
    let commands = if facts.all_commands {
        "every command line is admitted".to_string()
    } else if facts.command_patterns == 0 {
        "no command may be run at all".to_string()
    } else {
        format!("{} command pattern(s) admitted", facts.command_patterns)
    };
    format!(
        "## This session\n\nThe project root is {root}. Relative paths resolve against it.\n\n\
         The tools above are the whole set. To change part of a file, `read` it, edit the\n\
         text here in the cell, and `write` it back — you hold the file as an object, so a\n\
         replacement is `text.replace(a, b)` and not a shell command. `write` replaces the\n\
         whole file and creates parent directories. Check the command you intend is\n\
         admitted before you build a plan on `bash`.\n\n\
         Sandbox: {writable}; {commands}; network: {network}. Anything outside that throws\n\
         PermissionDenied, which is final — no cell widens a grant, so a refusal means\n\
         choose another route or say plainly that the grant forbids it.",
        root = facts.root,
        network = if facts.network { "yes" } else { "no" },
    )
}

/// §1's system block: the preamble, one declaration per tool in `tools`'
/// order, this session's own facts, then the project's own instructions.
pub fn render_system(instructions: &str, tools: &[&Tool], facts: &SessionFacts) -> String {
    let rendered: Vec<String> = tools.iter().map(|tool| render_declaration(tool)).collect();
    format!(
        "{PREAMBLE}\n\n## Tools\n\n{}\n\n## Runtime\n\n{}\n\n{}\n\n{instructions}",
        rendered.join("\n\n"),
        render_runtime(),
        render_session_facts(facts)
    )
}

/// §1's *Runtime* block: every host global that is not a tool.
///
/// The invariant is [`declarations::Binding`]'s: a name the isolate binds is
/// a name this block declares. It is rendered from the same table the
/// enumeration test checks, so the two cannot drift.
pub fn render_runtime() -> String {
    declarations::RUNTIME
        .iter()
        .map(|binding| binding.declaration.to_string())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_declaration(tool: &Tool) -> String {
    let entry = declarations::lookup(tool.name())
        .unwrap_or_else(|| panic!("no declarations entry for tool `{}`", tool.name()));
    format!(
        "declare function {name}(a: {params}): Promise<{ret}>;\n// {summary} {purity}\n// @callers program",
        name = tool.name(),
        params = render_params(tool.args()),
        ret = entry.return_type,
        summary = entry.summary,
        purity = declarations::purity_clause(tool.purity()),
    )
}

fn render_params(args: &[Arg]) -> String {
    if args.is_empty() {
        return "{}".to_string();
    }
    let fields: Vec<String> = args
        .iter()
        .map(|arg| {
            let optional_mark = if arg.is_required() { "" } else { "?" };
            format!("{}{optional_mark}: string", arg.name())
        })
        .collect();
    format!("{{{}}}", fields.join("; "))
}

/// One cell's outcome, as this module's own plain input — the wiring
/// package adapts the isolate's real outcome to it. `handle_table` is
/// already-rendered text; this module never builds one.
pub struct CellResult {
    pub cell: u64,
    pub elapsed_ms: u64,
    pub error: Option<ErrorSection>,
    /// Why the cell yielded on purpose — `runtime-contract.md` §9.3's one
    /// line under the cell line. Never rendered beside an error: a throw is
    /// not a yield, whatever else the caller filled in.
    pub yield_reason: Option<String>,
    pub handle_table: String,
    pub stdout_tail: Option<String>,
    pub budget: Budget,
    /// The model's own plan as the cell left it. Rendered as `## Plan` so a
    /// task longer than one cell is re-shown its checklist every turn, which
    /// is the whole reason a model keeps one.
    pub plan: Vec<PlanItem>,
}

/// §6's `## Error` section: the class, the message, where in the model's own
/// program it happened, and up to three in-program frames.
pub struct ErrorSection {
    pub class: String,
    pub message: String,
    /// The line and column inside the model's own program, when the runtime
    /// attributed the throw to one. Absent, no position line is written —
    /// never `line 0, column 0`, which names a place that does not exist.
    pub position: Option<(u64, u64)>,
    pub frames: Vec<String>,
}

impl ErrorSection {
    /// [`position`](Self::position) from a runtime error's own line and
    /// column, which is where the doc comment above is enforced.
    ///
    /// `line.zip(column)` alone is not enough: it distinguishes *absent*
    /// from *present*, and V8 reports a stack overflow as
    /// present-and-**zero**, so `function f(n) { return f(n + 1); } f(0)` —
    /// an ordinary bug in a model-written traversal — was rendered as
    /// `line 0, column 0`. There is no line 0 in anyone's program.
    pub fn position_of(line: Option<u32>, column: Option<u32>) -> Option<(u64, u64)> {
        line.zip(column)
            .filter(|&(line, column)| line != 0 || column != 0)
            .map(|(line, column)| (u64::from(line), u64::from(column)))
    }
}

/// The three figures §6's budget line reports.
pub struct Budget {
    pub turn_cap: u64,
    pub task_used: u64,
    pub task_cap: u64,
    pub cells_used: u64,
    pub cells_cap: u64,
}

/// §6's user message: the yield/throw line, then `## Handles`, `## Error`,
/// `## stdout` and `## Budget`, in that order, each omitted when there is
/// nothing to say — except `## Handles`, which is never omitted and writes
/// `(none)` for an empty table.
pub fn render_result(result: &CellResult) -> String {
    let verb = if result.error.is_some() {
        "threw"
    } else {
        "yielded"
    };
    let mut out = format!("[cell {} {verb} in {} ms]", result.cell, result.elapsed_ms);
    if result.error.is_none()
        && let Some(reason) = &result.yield_reason
    {
        out.push('\n');
        out.push_str(reason);
    }

    out.push_str("\n\n## Handles\n");
    if result.handle_table.is_empty() {
        out.push_str("(none)");
    } else {
        out.push_str(&result.handle_table);
    }

    if let Some(error) = &result.error {
        out.push_str("\n\n## Error\n");
        out.push_str(&format!("{}: {}", error.class, error.message));
        if let Some((line, column)) = error.position {
            out.push_str(&format!("\nline {line}, column {column}"));
        }
        for frame in error.frames.iter().take(3) {
            out.push_str(&format!("\n  at {frame}"));
        }
    }

    if !result.plan.is_empty() {
        out.push_str("\n\n## Plan\n");
        let rows: Vec<String> = result
            .plan
            .iter()
            .map(|item| format!("{} {}", item.status.mark(), item.text))
            .collect();
        out.push_str(&rows.join("\n"));
    }

    if let Some(stdout) = &result.stdout_tail {
        out.push_str("\n\n## stdout\n");
        out.push_str(stdout);
    }

    out.push_str("\n\n## Budget\n");
    out.push_str(&render_budget_line(&result.budget));

    out
}

fn render_budget_line(budget: &Budget) -> String {
    let mut line = format!(
        "turn cap {} · task {}/{} · cells {}/{}",
        thousands(budget.turn_cap),
        thousands(budget.task_used),
        thousands(budget.task_cap),
        thousands(budget.cells_used),
        thousands(budget.cells_cap),
    );
    if budget.task_cap > 0
        && budget.task_used.saturating_mul(100) >= budget.task_cap.saturating_mul(90)
    {
        line.push_str(" — finish or return");
    }
    line
}

/// `n` with a comma every three digits from the right — the budget line's
/// own formatting, `model-contract.md` §6's `8,000` / `3,412` / `400,000`.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(bytes.len() + bytes.len() / 3);
    for (i, byte) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*byte as char);
    }
    out
}

/// What one assistant message contained, per §5.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Extracted {
    /// Exactly one fenced block tagged `pane`; its source, unparsed.
    Program(String),
    /// One complete `pane-edit` fence: JSON amending a parse-failed cell.
    Edit(String),
    /// Two or more `pane` blocks in the same message. Neither runs.
    TwoBlocks,
    /// No `pane` block — including a message whose only fenced block is
    /// tagged something else, such as `ts`.
    Prose,
}

/// §5: the fence is three backticks at the start of a line, the info string
/// is the rest of that line trimmed, and a block ends at the next line that
/// is exactly three backticks.
pub fn extract_program(assistant_text: &str) -> Extracted {
    let lines: Vec<&str> = assistant_text.lines().collect();
    let mut programs = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if let Some(rest) = lines[i].strip_prefix("```") {
            let info = rest.trim();
            let mut body = Vec::new();
            let mut j = i + 1;
            while j < lines.len() && lines[j] != "```" {
                body.push(lines[j]);
                j += 1;
            }
            if info == "pane" {
                programs.push(Extracted::Program(body.join("\n")));
            } else if info == "pane-edit" {
                // An unfinished edit is invalid data, never an executable prefix.
                programs.push(Extracted::Edit(if j < lines.len() {
                    body.join("\n")
                } else {
                    String::new()
                }));
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    match programs.len() {
        0 => Extracted::Prose,
        1 => programs.into_iter().next().expect("length checked above"),
        _ => Extracted::TwoBlocks,
    }
}

// --- compaction: what a past turn still has to say ---------------------

/// What one pass of [`compact_conversation`] removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Compaction {
    /// Messages this pass shortened.
    pub messages: usize,
    /// Bytes removed from them.
    pub bytes: usize,
}

impl Compaction {
    pub fn is_empty(&self) -> bool {
        self.messages == 0
    }
}

/// The sections of a cell result that the **newest** result restates in full,
/// and which are therefore redundant in every older one.
///
/// The invariant: **a section listed here is rendered complete every turn, so
/// an older copy tells the model nothing the latest message does not.**
/// `## Handles` is the whole live table, `## Plan` the whole plan, `## Budget`
/// the current figures — each is a snapshot of now, not a record of then.
/// `## Error` and `## stdout` are the opposite: they belong to the cell that
/// produced them and appear nowhere else, so they are never dropped.
const SUPERSEDED_SECTIONS: [&str; 3] = ["## Handles", "## Plan", "## Budget"];

/// Removes the superseded sections from one rendered cell result.
///
/// Pure over its input, so the claim "this is lossless" is checked by
/// comparing against a freshly rendered result rather than by inspection.
pub fn compact_result(rendered: &str) -> String {
    let mut out = String::with_capacity(rendered.len());
    // Everything before the first section header is the cell line and its
    // yield reason, which name what happened and are kept.
    let mut parts = rendered.split("\n\n## ");
    if let Some(head) = parts.next() {
        out.push_str(head);
    }
    for section in parts {
        let name = section.split('\n').next().unwrap_or_default();
        if SUPERSEDED_SECTIONS
            .iter()
            .any(|superseded| superseded.trim_start_matches("## ") == name)
        {
            continue;
        }
        out.push_str("\n\n## ");
        out.push_str(section);
    }
    out
}

/// Whether `text` is a message pane rendered rather than one a person typed.
///
/// Only these are compacted: a person's own words are never edited, however
/// long the conversation gets.
pub fn is_rendered_result(text: &str) -> bool {
    text.starts_with("[cell ") || text.starts_with("## Handles")
}

/// Drops every superseded section from every rendered result **except the
/// most recent one**, which is the copy the others are redundant against.
///
/// Lossless by construction, and that is why it is the first thing tried: it
/// removes only text the conversation still carries somewhere else. When it
/// is not enough, [`checkpoint`] is the next rung and it is not lossless.
pub fn compact_conversation(conversation: &mut Conversation) -> Compaction {
    let last_rendered = conversation.messages.iter().rposition(|message| {
        message.content.iter().any(|block| match block {
            Block::Text(text) => is_rendered_result(text),
        })
    });
    let Some(last_rendered) = last_rendered else {
        return Compaction::default();
    };

    let mut report = Compaction::default();
    for (index, message) in conversation.messages.iter_mut().enumerate() {
        if index == last_rendered {
            continue;
        }
        for block in message.content.iter_mut() {
            let Block::Text(text) = block;
            if !is_rendered_result(text) {
                continue;
            }
            let compacted = compact_result(text);
            if compacted.len() < text.len() {
                report.messages += 1;
                report.bytes += text.len() - compacted.len();
                *text = compacted;
            }
        }
    }
    report
}

/// The second rung: what one task hands itself when its conversation has to
/// be thrown away.
///
/// The invariant, and the reason pane can do this at all: **the working set
/// survives the compaction.** A text harness's tool results *are* the
/// transcript, so discarding the transcript discards them; pane's live
/// objects are in the isolate and are re-listed in the next cell's handle
/// table, so what is lost here is the narration and not the work. The
/// checkpoint therefore names the handles rather than describing them — the
/// table that follows it is authoritative and complete.
///
/// Its four parts are `docs/process/` checkpoints' own: what the task was,
/// where it got to, what it ruled out, what to do next.
pub fn checkpoint(
    task: &str,
    plan: &[PlanItem],
    live_handles: &[String],
    last_error: Option<&str>,
) -> String {
    let mut out = String::new();
    out.push_str(
        "The conversation before this point was dropped because it no longer fit. **Your \
         objects are untouched** — every handle below is live, and the handle table in the \
         next result is complete. Nothing needs re-reading or re-running.\n\n",
    );
    out.push_str("## The task\n");
    out.push_str(task.trim());

    if plan.is_empty() {
        out.push_str(
            "\n\n## Where you got to\nNo plan was written before the drop. Write one with \
             `todo.write` before going further, so the next drop has something to carry.",
        );
    } else {
        out.push_str("\n\n## Where you got to\n");
        let rows: Vec<String> = plan
            .iter()
            .map(|item| format!("{} {}", item.status.mark(), item.text))
            .collect();
        out.push_str(&rows.join("\n"));
    }

    if let Some(error) = last_error {
        out.push_str("\n\n## What went wrong last\n");
        out.push_str(error.trim());
    }

    out.push_str("\n\n## What you still hold\n");
    if live_handles.is_empty() {
        out.push_str("Nothing yet.");
    } else {
        out.push_str(&live_handles.join(", "));
    }
    out
}
