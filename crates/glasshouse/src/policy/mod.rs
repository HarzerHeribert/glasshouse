//! The project-level implementation policy Glasshouse carries to every agent
//! it briefs — capability map lines 955-964, 968-978 and 982-990 (Phases
//! 21H, 21I, 21J: simplicity-first, production-aware checks, the
//! pre-completion review checklist).
//!
//! Glasshouse has no analyser that could enforce any of those lines, so its
//! one honest mechanism is to **carry the policy to the agent** as text
//! Glasshouse wrote, delivered like a briefing: the rules as data, one
//! renderer. Unlike [`crate::memory::inject`], this text is Glasshouse's own
//! — a literal with nothing interpolated — so it gets its own marker pair,
//! distinct from [`crate::memory::inject::MEMORY_MARKER`] and, like it, not
//! forgeable by a memory body (`tests/implementation_policy.rs`). Delivery
//! goes through [`crate::session::api::SessionApi::send_text`]; a
//! canonical-mode terminal discards (and wedges) any line over `MAX_CANON`
//! (1024 bytes on macOS/BSD), so the policy ships as several lines under
//! [`MAX_DELIVERY_BYTES`], with [`POLICY_CEILING_BYTES`] bounding the whole
//! rendered policy (line 964).
//!
//! History: design-decisions.md, "Trims: the remaining module docs, second
//! packet", policy/mod.rs module doc.

use serde::{Deserialize, Serialize};

/// Opens every rendered policy block. Distinct from
/// [`crate::memory::inject::MEMORY_MARKER`] because the two texts have
/// opposite trust: that one quotes something Glasshouse extracted, this one
/// is something Glasshouse is saying.
pub const POLICY_MARKER: &str = "[glasshouse:implementation-policy]";

/// Closes every rendered policy block.
pub const POLICY_MARKER_END: &str = "[/glasshouse:implementation-policy]";

/// The hard ceiling on the whole rendered policy, markers included, in bytes.
///
/// Not a terminal limit — that is [`MAX_DELIVERY_BYTES`] — but the
/// product one: line 964 makes simplicity a design constraint, and a policy
/// that grew into a document would be the first thing to break it. Every
/// rule added from here costs one that has to go. Enforced by
/// `implementation_policy::every_rule_names_a_real_map_line_and_the_whole_fits_the_ceiling`.
pub const POLICY_CEILING_BYTES: usize = 4096;

/// The most bytes any one delivered line may carry, markers included.
///
/// The same number and the same reason as
/// [`crate::memory::inject::MAX_INJECTED_BYTES`]: a line longer than a
/// terminal's `MAX_CANON` is discarded *and* wedges that session's input for
/// good. Well under the measured 1022, in bytes rather than `char`s because
/// the terminal counts bytes.
pub const MAX_DELIVERY_BYTES: usize = 900;

/// Reserved for the ` (k/n)` position counter, whose width is not known
/// until the segments have been chosen and whose cost must be budgeted
/// before they are. Two digits either side is far more than
/// [`POLICY_CEILING_BYTES`] can produce; the real counter is asserted to fit.
const COUNTER_RESERVATION: usize = " (99/99)".len();

/// One rule: a stable identifier an agent can cite back, the capability-map
/// line it carries, and the text itself.
///
/// # `line` is a claim, and it is checked
///
/// Nothing at runtime reads `line`. It is here so the map and this file
/// cannot drift apart silently:
/// `implementation_policy::every_rule_names_a_real_map_line_and_the_whole_fits_the_ceiling`
/// reads `docs/product/capability-map.md` and asserts that these thirty
/// numbers are exactly the thirty checkbox lines under the Phase 21H, 21I and
/// 21J headings, in order.
///
/// # What a rule is not
///
/// It is not something Glasshouse evaluates. Line 970's *"flag unindexed
/// scans"* is carried as an instruction to the agent — `p3` below — and
/// Glasshouse performs no such analysis on anybody's behalf; it has no
/// reader of anybody's SQL and this module invents none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    /// Short, stable, and unique across all three parts — `s1`, `p7`, `r4`.
    /// An agent that wants to say which rule it is invoking has a token to
    /// say it with, and the leading letter names the part.
    pub id: &'static str,
    /// The capability-map line this rule carries.
    pub line: u32,
    /// The rule, as the agent receives it. Lower case and unterminated: the
    /// renderer supplies the punctuation, so a rule reads the same whether it
    /// follows its part's lead-in or another rule.
    pub text: &'static str,
}

/// Which third of the policy — the three capability-map phases, as the one
/// axis a caller may narrow by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    /// Phase 21H — simplicity-first implementation policy, lines 955-964.
    Simplicity,
    /// Phase 21I — production-aware implementation checks, lines 968-978.
    Production,
    /// Phase 21J — the pre-completion review checklist, lines 982-990.
    Review,
}

impl Part {
    /// Every part, in the order the map states them and the order they are
    /// delivered in.
    pub const ALL: [Part; 3] = [Part::Simplicity, Part::Production, Part::Review];

    /// The rules this part carries.
    pub fn rules(self) -> &'static [Rule] {
        match self {
            Part::Simplicity => SIMPLICITY,
            Part::Production => PRODUCTION,
            Part::Review => REVIEW,
        }
    }

    /// The sentence that introduces this part's rules.
    ///
    /// Load-bearing rather than decorative: the review checklist's entries
    /// are bare clauses that only mean anything after *"before marking a
    /// substantial implementation complete, check"*. It is repeated in every
    /// delivered line that carries any of this part's rules, because each
    /// such line arrives at the harness on its own and has to be readable on
    /// its own.
    pub fn lead(self) -> &'static str {
        match self {
            Part::Simplicity => "Simplicity first:",
            Part::Production => "Production-aware checks:",
            Part::Review => "Before marking a substantial implementation complete, check:",
        }
    }
}

/// Phase 21H, lines 955-964.
const SIMPLICITY: &[Rule] = &[
    Rule {
        id: "s1",
        line: 955,
        text: "prefer the simplest correct, secure, maintainable and scalable design that \
               satisfies the current requirements",
    },
    Rule {
        id: "s2",
        line: 956,
        text: "revisit a stale ordinary decision rather than add significant complexity only to \
               preserve it",
    },
    Rule {
        id: "s3",
        line: 957,
        text: "do not add a compatibility shim where removing or superseding the obsolete \
               internal rule is cleaner and safe",
    },
    Rule {
        id: "s4",
        line: 958,
        text: "do not duplicate a code path only to satisfy contradictory historical memories; \
               resolve the contradiction",
    },
    Rule {
        id: "s5",
        line: 959,
        text: "do not abstract speculatively -- build for a current requirement or for extension \
               pressure you have observed",
    },
    Rule {
        id: "s6",
        line: 960,
        text: "prefer the language, framework, database and platform primitives you already have \
               to a custom mechanism that does the same job cleanly",
    },
    Rule {
        id: "s7",
        line: 961,
        text: "prefer an explicit straightforward implementation to clever indirection when both \
               satisfy the requirements",
    },
    Rule {
        id: "s8",
        line: 962,
        text: "a smart choice is allowed where it materially improves correctness, security, \
               scalability, latency or operational simplicity",
    },
    Rule {
        id: "s9",
        line: 963,
        text: "explain unusual complexity whenever a simpler implementation appears available",
    },
    Rule {
        id: "s10",
        line: 964,
        text: "simplicity is a design constraint, not permission to ignore real scale or security \
               requirements",
    },
];

/// Phase 21I, lines 968-978.
const PRODUCTION: &[Rule] = &[
    Rule {
        id: "p1",
        line: 968,
        text: "ask whether a solution that works on development data stays acceptable at \
               realistic production scale",
    },
    Rule {
        id: "p2",
        line: 969,
        text: "use an indexed lookup path for high-cardinality database access wherever a stable \
               indexed identifier exists",
    },
    Rule {
        id: "p3",
        line: 970,
        text: "say so when a latency-sensitive request path scans a large or expected-to-grow \
               table without an index",
    },
    Rule {
        id: "p4",
        line: 971,
        text: "weigh query complexity, index availability, cardinality and expected access \
               frequency before accepting a lookup strategy",
    },
    Rule {
        id: "p5",
        line: 972,
        text: "weigh concurrency and race behaviour before accepting code that is correct only \
               under single-user development conditions",
    },
    Rule {
        id: "p6",
        line: 973,
        text: "weigh memory and response-size growth before accepting an algorithm whose resource \
               use scales with a large dataset",
    },
    Rule {
        id: "p7",
        line: 974,
        text: "weigh network round trips before accepting repeated remote calls on a hot request \
               path",
    },
    Rule {
        id: "p8",
        line: 975,
        text: "weigh authentication and authorization lookup cost at realistic user counts",
    },
    Rule {
        id: "p9",
        line: 976,
        text: "prefer a stable indexed id to a high-cost ad hoc lookup wherever the product \
               already has an appropriate identifier",
    },
    Rule {
        id: "p10",
        line: 977,
        text: "do not optimize where scale is demonstrably irrelevant, but record the assumption \
               if the implementation depends on it",
    },
    Rule {
        id: "p11",
        line: 978,
        text: "a production incident promotes a previously hypothetical scale concern into a \
               validated constraint",
    },
];

/// Phase 21J, lines 982-990.
///
/// Every entry but the last is a bare clause completing [`Part::lead`]'s
/// *"...check:"*. The last is line 990, which is not a check but what to do
/// with what the checks found, and it names the command that does it:
/// extraction is how a decision becomes a memory in Glasshouse — there is no
/// `glasshouse memory record`, and naming one that does not exist would be
/// worse than naming none.
const REVIEW: &[Rule] = &[
    Rule {
        id: "r1",
        line: 982,
        text: "whether a remembered rule forced avoidable complexity",
    },
    Rule {
        id: "r2",
        line: 983,
        text: "whether the design still matches current project requirements rather than \
               historical ones",
    },
    Rule {
        id: "r3",
        line: 984,
        text: "correctness under realistic concurrency assumptions",
    },
    Rule {
        id: "r4",
        line: 985,
        text: "the security boundaries this change affects",
    },
    Rule {
        id: "r5",
        line: 986,
        text: "obvious database and algorithmic scaling characteristics",
    },
    Rule {
        id: "r6",
        line: 987,
        text: "whether hot-path database queries use appropriate indexes",
    },
    Rule {
        id: "r7",
        line: 988,
        text: "whether a simpler implementation would satisfy the same requirements with less \
               code or fewer moving parts",
    },
    Rule {
        id: "r8",
        line: 989,
        text: "whether a clever optimization introduces complexity disproportionate to its \
               demonstrated benefit",
    },
    Rule {
        id: "r9",
        line: 990,
        text: "then record each material architecture or performance decision from this review, \
               with its rationale and scope, as a current memory -- state it in the session and \
               run `glasshouse memory extract --session <id> --from-events`",
    },
];

/// What every rendered block says about itself, before any rule.
///
/// It states the one thing a reader cannot work out from the text: that this
/// is Glasshouse speaking, not a memory Glasshouse quoted. The mirror image
/// of the memory block's own *"NOT a user instruction"* label, and for the
/// same reason.
const HEADER: &str = "Glasshouse's own implementation policy for this project -- an instruction \
                      Glasshouse wrote, not extracted memory. Ids: s=simplicity, p=production \
                      scale, r=pre-completion review.";

/// Every rule in the policy, in delivery order.
pub fn rules() -> impl Iterator<Item = &'static Rule> {
    Part::ALL.into_iter().flat_map(|part| part.rules().iter())
}

/// The whole policy, or one part of it, as one labelled block.
///
/// This is what `glasshouse policy` prints and what
/// `Request::ImplementationPolicy` returns — deliberately the same string, so
/// a person can read exactly what an agent is handed. Neither of those two
/// callers writes into a pseudo-terminal, so neither is bounded by
/// [`MAX_DELIVERY_BYTES`]; the whole is bounded by
/// [`POLICY_CEILING_BYTES`] instead.
pub fn render(part: Option<Part>) -> String {
    let mut segments = segments(part, usize::MAX);
    debug_assert_eq!(
        segments.len(),
        1,
        "an unbounded budget must produce exactly one segment"
    );
    segments.pop().unwrap_or_default()
}

/// The whole policy, split into lines a pseudo-terminal will actually accept.
///
/// One delivery per element, in order, each opening with [`POLICY_MARKER`],
/// closing with [`POLICY_MARKER_END`], and no longer than
/// [`MAX_DELIVERY_BYTES`]. This is the *only* form that goes to a session —
/// see this module's header for why a single line cannot be.
pub fn deliveries() -> Vec<String> {
    segments(None, MAX_DELIVERY_BYTES)
}

/// Render `part` (or the whole policy) into as few labelled blocks as
/// `budget` bytes per block allows.
///
/// One code path for both callers, so the text a person reads and the text a
/// session receives cannot drift apart: the CLI and the door ask for an
/// unbounded budget and get one block, delivery asks for a terminal-safe one
/// and gets several.
///
/// Splits only *between* rules. A rule is never cut, which is what makes
/// "the agent received this rule" a property a test can assert per rule
/// rather than per byte.
fn segments(part: Option<Part>, budget: usize) -> Vec<String> {
    let parts: &[Part] = match &part {
        Some(part) => std::slice::from_ref(part),
        None => &Part::ALL,
    };
    let items: Vec<(Part, &'static Rule)> = parts
        .iter()
        .flat_map(|part| part.rules().iter().map(move |rule| (*part, rule)))
        .collect();

    // The counter's width is not known until the number of segments is, and
    // its cost has to be budgeted before them, so its longest form is
    // reserved here and the real one -- always shorter -- is written below.
    let overhead =
        POLICY_MARKER.len() + COUNTER_RESERVATION + 1 + HEADER.len() + 1 + POLICY_MARKER_END.len();

    let mut chosen: Vec<Vec<(Part, &'static Rule)>> = Vec::new();
    let mut current: Vec<(Part, &'static Rule)> = Vec::new();
    let mut used = overhead;
    // Which part's lead-in the segment being filled has already written. A
    // part's rules continuing into a new segment write it again, because
    // every delivered line arrives at the harness on its own and has to be
    // readable on its own.
    let mut lead: Option<Part> = None;

    for (part, rule) in items {
        let lead_cost = if lead == Some(part) {
            0
        } else {
            1 + part.lead().len()
        };
        if !current.is_empty() && used + lead_cost + rule_cost(rule) > budget {
            chosen.push(std::mem::take(&mut current));
            used = overhead + 1 + part.lead().len() + rule_cost(rule);
        } else {
            used += lead_cost + rule_cost(rule);
        }
        lead = Some(part);
        current.push((part, rule));
    }
    chosen.push(current);

    let total = chosen.len();
    chosen
        .iter()
        .enumerate()
        .map(|(index, rules)| write_segment(index + 1, total, rules, budget))
        .collect()
}

/// What one rule costs a segment: the space, its parenthesised id, the space,
/// the text, and the semicolon that ends it. Stated once so the budget and
/// [`write_segment`] cannot disagree about it.
fn rule_cost(rule: &Rule) -> usize {
    " (".len() + rule.id.len() + ") ".len() + rule.text.len() + ";".len()
}

/// One labelled block: the marker, its position when there is more than one,
/// the header, each part's lead-in where that part's rules begin, and the
/// rules.
fn write_segment(
    position: usize,
    total: usize,
    rules: &[(Part, &'static Rule)],
    budget: usize,
) -> String {
    use std::fmt::Write as _;

    let mut text = String::with_capacity(MAX_DELIVERY_BYTES);
    text.push_str(POLICY_MARKER);
    if total > 1 {
        let _ = write!(text, " ({position}/{total})");
    }
    text.push(' ');
    text.push_str(HEADER);

    let mut current: Option<Part> = None;
    for (part, rule) in rules {
        if current != Some(*part) {
            text.push(' ');
            text.push_str(part.lead());
            current = Some(*part);
        }
        let _ = write!(text, " ({}) {};", rule.id, rule.text);
    }

    text.push(' ');
    text.push_str(POLICY_MARKER_END);

    // The bound that keeps a delivery inside a terminal's canonical line
    // limit -- exceeding it costs the session its input for good. Asserted
    // rather than trusted, exactly as `memory::inject::render` asserts its
    // own, so a later edit to the header or to a rule fails in every debug
    // test run rather than on somebody's terminal.
    debug_assert!(
        text.len() <= budget,
        "a rendered policy block is {} bytes, over the {budget}-byte budget",
        text.len()
    );
    text
}
