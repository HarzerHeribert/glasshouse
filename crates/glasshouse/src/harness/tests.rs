use super::*;

/// Production source of a module, with its test module and its comments
/// removed.
///
/// Comments are stripped because the architectural rules below are about
/// what the code *depends on*, not what its prose mentions. `session/store`
/// documents that it holds an `IntegrationId`'s string form, which is the
/// architecture working, not breaking it — a scan that could not tell those
/// apart would punish the comment that explains the boundary.
fn production_code(source: &str) -> String {
    source
        .split("#[cfg(test)]")
        .next()
        .expect("split always yields at least one part")
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with("//")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `session/store`'s production code, joined -- Phase 59
/// (`GH-DECOMP-SESSION-STORE`) split `session/store.rs` into
/// `mod.rs`, `record.rs` and `context.rs`, as `routing/mod.rs::session_source`
/// joins `routing/session`'s files.
///
/// `production_code` is applied to each file **before** the join, not
/// after: `mod.rs`'s own `#[cfg(test)] mod tests;` declaration sits after
/// every other item in that file (deliberately, for exactly this scan),
/// so truncating each file on its own first-and-only `#[cfg(test)]`
/// keeps every production line of all three files; truncating the joined
/// string once would stop at `mod.rs`'s marker and silently drop
/// `record.rs` and `context.rs` from the scan.
fn session_store_source() -> String {
    [
        production_code(include_str!("../session/store/record.rs")),
        production_code(include_str!("../session/store/context.rs")),
        production_code(include_str!("../session/store/mod.rs")),
    ]
    .join("\n")
}

// --- the catalogue and its adapters ---------------------------------

#[test]
fn every_harness_has_an_adapter_and_nothing_else_does() {
    for &id in IntegrationId::ALL {
        match id.kind() {
            IntegrationKind::Harness => {
                let adapter = adapter_for(id)
                    .unwrap_or_else(|| panic!("{} is a harness with no adapter", id.slug()));
                assert_eq!(
                    adapter.id(),
                    id,
                    "the adapter registered for {} reports a different identity",
                    id.slug()
                );
            }
            IntegrationKind::Multiplexer | IntegrationKind::LocalInference => {
                assert!(
                    adapter_for(id).is_none(),
                    "{} is not a harness but has an adapter",
                    id.slug()
                );
            }
        }
    }
}

#[test]
fn all_lists_exactly_the_harness_adapters() {
    let listed: Vec<IntegrationId> = all().map(|a| a.id()).collect();
    let harnesses: Vec<IntegrationId> = IntegrationId::ALL
        .iter()
        .copied()
        .filter(|id| id.kind() == IntegrationKind::Harness)
        .collect();
    assert_eq!(listed, harnesses);
}

// --- executable presence (Phase 9F line 466) --------------------------

fn not_found(name: &str) -> crate::platform::exec::ResolveError {
    crate::platform::exec::ResolveError::NotFound {
        name: name.to_owned(),
    }
}

/// A usable resolution, real enough to construct: this test binary's own
/// path always resolves as one via
/// [`crate::platform::exec::resolve_explicit`].
fn usable() -> crate::platform::exec::ResolvedExecutable {
    crate::platform::exec::resolve_explicit(&std::env::current_exe().expect("a test binary"))
        .expect("the running test binary resolves as usable")
}

#[test]
fn a_candidate_that_resolves_is_usable() {
    let presence = ExecutablePresence::detect_with(&["claude", "claude-code"], |_| Ok(usable()));
    assert_eq!(presence, ExecutablePresence::Usable);
    assert!(presence.is_usable());
}

#[test]
fn every_candidate_not_found_is_not_found_and_lists_every_candidate_tried() {
    let presence =
        ExecutablePresence::detect_with(&["claude", "claude-code"], |name| Err(not_found(name)));
    assert_eq!(presence, ExecutablePresence::NotFound);
    assert!(!presence.is_usable());
    assert_eq!(
        presence.detail(IntegrationId::ClaudeCode),
        format!(
            "candidates tried: {}",
            IntegrationId::ClaudeCode.executable_candidates().join(", ")
        )
    );
}

/// A found-but-unusable hit outranks a later plain miss — the same
/// priority `integrations::resolve_first_usable_with` gives it, and for
/// the same reason: it is a more specific, more actionable finding.
#[test]
fn a_found_but_unusable_candidate_outranks_a_later_not_found() {
    let presence = ExecutablePresence::detect_with(&["claude", "claude-code"], |name| {
        if name == "claude" {
            Err(crate::platform::exec::ResolveError::NotExecutable {
                path: PathBuf::from("/opt/claude"),
            })
        } else {
            Err(not_found(name))
        }
    });
    match &presence {
        ExecutablePresence::Unusable { reason } => {
            assert!(reason.contains("/opt/claude"), "{reason}");
        }
        other => panic!("expected Unusable, got {other:?}"),
    }
    assert!(!presence.is_usable());
}

#[test]
fn no_two_adapters_claim_the_same_executable_name() {
    let mut seen: Vec<(&str, IntegrationId)> = Vec::new();
    for adapter in all() {
        for &name in adapter.executable_candidates() {
            if let Some((_, other)) = seen.iter().find(|(n, _)| *n == name) {
                panic!(
                    "`{name}` is claimed by both {} and {}: PATH discovery would resolve \
                     one harness as the other",
                    other.slug(),
                    adapter.id().slug()
                );
            }
            seen.push((name, adapter.id()));
        }
    }
}

#[test]
fn every_adapter_names_at_least_one_executable() {
    for adapter in all() {
        let candidates = adapter.executable_candidates();
        assert!(
            !candidates.is_empty(),
            "{} names no executable, so it can never be found",
            adapter.id().slug()
        );
        for name in candidates {
            assert!(!name.trim().is_empty());
            assert!(
                !name.contains(std::path::MAIN_SEPARATOR),
                "{} names `{name}`, which is a path and not a PATH-searchable name",
                adapter.id().slug()
            );
        }
    }
}

/// The catalogue must ask the adapter, not keep its own copy.
#[test]
fn the_catalogue_takes_harness_executable_names_from_the_adapter() {
    for adapter in all() {
        assert_eq!(
            adapter.id().executable_candidates(),
            adapter.executable_candidates(),
            "{} would be searched for under a different name than its adapter declares",
            adapter.id().slug()
        );
    }
}

// --- declarations are evidence --------------------------------------

#[test]
fn every_verified_declaration_cites_its_evidence() {
    // A `Verified` with an empty evidence string is the exact failure this
    // type exists to prevent: a claim with nothing behind it, wearing the
    // word "verified".
    fn check(what: &str, evidence: Option<&'static str>) {
        if let Some(evidence) = evidence {
            assert!(
                evidence.trim().len() > 20,
                "{what} is declared verified but cites no usable evidence: {evidence:?}"
            );
        }
    }

    for adapter in all() {
        let slug = adapter.id().slug();
        let d = adapter.describe();
        check(&format!("{slug} vendor"), d.vendor.evidence());
        check(&format!("{slug} hooks"), d.hooks.evidence());
        check(&format!("{slug} session ids"), d.session_ids.evidence());
        check(
            &format!("{slug} protocols"),
            d.backends.protocols.evidence(),
        );
        check(
            &format!("{slug} model override"),
            d.backends.model_override.evidence(),
        );
        check(
            &format!("{slug} backend selection"),
            d.backends.selection.evidence(),
        );
        check(
            &format!("{slug} communication style"),
            d.communication_style.evidence(),
        );
        check(
            &format!("{slug} automatic review"),
            d.approvals.automatic_review.evidence(),
        );
        check(&format!("{slug} bypass"), d.approvals.bypass.evidence());
        check(&format!("{slug} sandbox"), d.approvals.sandbox.evidence());
        for (name, declared) in d.capabilities.named() {
            check(&format!("{slug} {name}"), declared.evidence());
        }
    }
}

#[test]
fn every_adapter_declares_its_native_communication_style_and_session_cost() {
    // The declaration lives in `HarnessDescription`, like
    // `HookInstallation`: an adapter cannot quietly omit the question.
    // Keep the full table exact. In particular, a launch-time mechanism
    // must not be represented as an in-place change merely because it is
    // convenient for a future caller; doing so would lose a warm native
    // session without warning.
    let table: Vec<(IntegrationId, Option<CommunicationStyle>)> = all()
        .map(|adapter| {
            (
                adapter.id(),
                adapter.describe().communication_style.value().copied(),
            )
        })
        .collect();

    assert_eq!(
        table,
        vec![
            (
                IntegrationId::ClaudeCode,
                Some(CommunicationStyle {
                    mechanism: "output style, supplied in the settings document passed with \
                                `--settings` when the session starts",
                    change: StyleChange::NewSession,
                    cache_invalidation: Declared::verified(
                        CacheInvalidation::Partial { one_turn: true },
                        "Claude Code 2.1.252 (2026-09-01): a session resumed with `--settings \
                         '{\"outputStyle\": \"<name>\"}'` or with `--append-system-prompt \
                         \"<text>\"` shows `cache_read_input_tokens` drop from the prior \
                         turn's level (~28,800-30,100) to ~18,500 and \
                         `cache_creation_input_tokens` rise from an undisturbed residual of \
                         57-432 to 13,300-13,960 on that turn, reproduced in 2 runs per \
                         mechanism (4 runs total) against a 2-run no-change control; the \
                         effect is partial (a base cache segment survives) and lasts exactly \
                         one turn. Changing the output style materially invalidates the \
                         prompt cache.",
                    ),
                }),
            ),
            (IntegrationId::Codex, None),
            (IntegrationId::Antigravity, None),
            (IntegrationId::OpenCode, None),
            (IntegrationId::Cursor, None),
            (IntegrationId::Pi, None),
            (
                IntegrationId::Hermes,
                Some(CommunicationStyle {
                    mechanism: "personality overlay, selected with `/personality <name>` \
                                inside a running session and stored as the \
                                `display.personality` configuration key",
                    change: StyleChange::InPlace,
                    cache_invalidation: Declared::Unverified,
                }),
            ),
        ],
        "read any changed declaration from the named native artifact before changing this \
         table; unsupported or unobserved style mechanisms must remain unknown"
    );
}

// --- approvals: honesty about review vs. bypass ----------------------

#[test]
fn each_adapter_declares_the_approval_mode_its_binary_documents() {
    // Exact, not a proxy. An earlier version of this test asserted only
    // that an `automatic_review` evidence string avoided the words "yolo",
    // "dangerously" and "bypass" — and a mutation walked straight through
    // it, recording OpenCode's blanket `--auto` as automatic review with
    // evidence reading "auto-approve permissions that are not explicitly
    // denied (dangerous!)". "dangerous!" is not "dangerously", so the
    // substring check passed and the wrong claim stood.
    //
    // The property worth holding is not how a declaration is *worded*, it
    // is *which mode each harness actually has*. Three do; four do not,
    // and one of those four could not be read at all. Pinning the table
    // — now the argv itself, not just a description — makes both halves
    // unfoolable.
    let table: Vec<(IntegrationId, Option<&'static [&'static str]>)> = all()
        .map(|adapter| {
            (
                adapter.id(),
                adapter
                    .describe()
                    .approvals
                    .automatic_review
                    .value()
                    .map(|mode| mode.args),
            )
        })
        .collect();

    assert_eq!(
        table,
        vec![
            (
                IntegrationId::ClaudeCode,
                Some(&["--permission-mode", "auto"][..])
            ),
            (IntegrationId::Codex, Some(&["--approve-for-me"][..])),
            (IntegrationId::Antigravity, None),
            (IntegrationId::OpenCode, None),
            (IntegrationId::Cursor, Some(&["--auto-review"][..])),
            (IntegrationId::Pi, None),
            (IntegrationId::Hermes, None),
        ],
        "an adapter's automatic-review declaration changed; if a harness \
         really gained or lost one, read it from the binary and update this \
         table with the evidence"
    );
}

#[test]
fn claude_code_selects_auto_mode_with_a_session_flag_not_the_subcommand() {
    // `auto-mode` is a Claude Code *subcommand* — "Inspect or reset auto
    // mode classifier configuration" — and appending it to a launch would
    // run that subcommand instead of starting a session. The flag that
    // actually selects the mode for a session is `--permission-mode auto`.
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let mode = adapter
        .describe()
        .approvals
        .automatic_review
        .value()
        .copied()
        .expect("Claude Code declares automatic review");
    assert_eq!(mode.args, &["--permission-mode", "auto"]);
    assert!(
        !mode.args.contains(&"auto-mode"),
        "`auto-mode` is a subcommand that inspects the classifier's \
         configuration; it does not start a session, so it must never \
         appear in the argv that selects automatic review"
    );
}

#[test]
fn no_approval_description_contains_a_backtick() {
    // `glasshouse doctor` renders each description already wrapped in
    // backticks, so a description carrying one of its own produces a
    // doubled, broken row: `auto review ``--permission-mode auto` — ...`
    // was exactly what the binary printed before this test existed. Found
    // by running the binary, which is the only way this class of defect
    // ever shows up — the types are all perfectly well-formed.
    for adapter in all() {
        let described = adapter.describe();
        for (label, declared) in [
            ("automatic_review", described.approvals.automatic_review),
            ("bypass", described.approvals.bypass),
        ] {
            let Some(mode) = declared.value() else {
                continue;
            };
            assert!(
                !mode.description.contains('`'),
                "{} {label} description {:?} contains a backtick; the doctor \
                 report wraps descriptions in backticks, so this renders doubled",
                mode.description,
                adapter.id().slug()
            );
        }
    }
}

#[test]
fn no_approval_argument_is_a_usage_string_rather_than_an_argv_entry() {
    // This is the check that would have caught `-s/--sandbox
    // <read-only|...>` being handed to a process as one argument: a usage
    // string with a placeholder is not an argv entry, and a space inside
    // one element means it was never meant to be passed as one.
    for adapter in all() {
        let described = adapter.describe();
        for (label, declared) in [
            ("automatic_review", described.approvals.automatic_review),
            ("bypass", described.approvals.bypass),
        ] {
            let Some(mode) = declared.value() else {
                continue;
            };
            for arg in mode.args {
                assert!(
                    !arg.contains(' ')
                        && !arg.contains('<')
                        && !arg.contains('>')
                        && !arg.contains('|'),
                    "{} {label} argument {arg:?} looks like a usage string, not an argv entry",
                    adapter.id().slug()
                );
            }
        }
    }
}

#[test]
fn a_harness_without_automatic_review_offers_no_substitute() {
    // OpenCode, Hermes and Antigravity each declare a bypass alongside
    // their unverified automatic review; for those three, `approval_args`
    // must not silently hand back the bypass argv when automatic review is
    // asked for. Pi declares neither (its whole `ApprovalModes` is
    // `UNVERIFIED`), so there is nothing to substitute in the first place
    // — the comparison is skipped rather than made vacuously against
    // `None == None`.
    for id in [
        IntegrationId::OpenCode,
        IntegrationId::Hermes,
        IntegrationId::Antigravity,
        IntegrationId::Pi,
    ] {
        let adapter = adapter_for(id).expect("a harness");
        let automatic = adapter.approval_args(ApprovalKind::AutomaticReview);
        assert_eq!(
            automatic,
            None,
            "{} declares automatic review it should not have",
            id.slug()
        );
        let bypass = adapter.approval_args(ApprovalKind::Bypass);
        if bypass.is_some() {
            assert_ne!(
                automatic,
                bypass,
                "{} must not substitute its bypass argv for a missing automatic \
                 review mode",
                id.slug()
            );
        }
    }
}

#[test]
fn three_harnesses_declare_automatic_review() {
    // Pinned so a future adapter cannot quietly claim parity with a
    // harness's real automatic-review mode without evidence.
    let declaring: Vec<IntegrationId> = all()
        .filter(|adapter| adapter.describe().approvals.has_automatic_review())
        .map(|adapter| adapter.id())
        .collect();
    assert_eq!(
        declaring,
        vec![
            IntegrationId::ClaudeCode,
            IntegrationId::Codex,
            IntegrationId::Cursor,
        ]
    );
}

#[test]
fn a_verified_hook_mechanism_is_never_empty() {
    for adapter in all() {
        if let Some(hooks) = adapter.describe().hooks.value() {
            assert!(
                !hooks.mechanism.trim().is_empty(),
                "{} declares hooks with no mechanism to configure them",
                adapter.id().slug()
            );
        }
    }
}

#[test]
fn a_verified_backend_declaration_is_never_an_empty_list() {
    for adapter in all() {
        let backends = adapter.describe().backends;
        if let Some(protocols) = backends.protocols.value() {
            assert!(!protocols.is_empty(), "{}", adapter.id().slug());
        }
        if let Some(overrides) = backends.model_override.value() {
            assert!(!overrides.is_empty(), "{}", adapter.id().slug());
        }
        if let Some(selection) = backends.selection.value() {
            assert!(!selection.is_empty(), "{}", adapter.id().slug());
        }
    }
}

#[test]
fn unverified_declarations_carry_no_value_and_no_evidence() {
    let unverified: Declared<Vendor> = Declared::Unverified;
    assert!(unverified.value().is_none());
    assert!(unverified.evidence().is_none());
    assert!(!unverified.is_verified());
}

#[test]
fn an_unverified_capability_is_not_treated_as_present() {
    let unverified: Declared<bool> = Declared::Unverified;
    assert!(!unverified.is_known_present());
    let absent = Declared::verified(false, "checked and it is not there");
    assert!(!absent.is_known_present());
    let present = Declared::verified(true, "checked and it is there");
    assert!(present.is_known_present());
}

// --- starting and resuming ------------------------------------------

#[test]
fn no_supported_harness_needs_an_argument_to_start_today() {
    // Every one of them opens an interactive session when run bare, and
    // Glasshouse has already put the child in the project root. If this
    // ever stops being true for a harness, that is a decision to make
    // deliberately rather than to discover in a session that came up
    // wrong.
    for adapter in all() {
        assert!(
            adapter.start().is_bare(),
            "{} now needs a start argument; update its adapter and this test together",
            adapter.id().slug()
        );
    }
}

#[test]
fn resume_passes_the_identifier_as_one_whole_argument() {
    // Glued on with `=`, an identifier beginning with a dash could be
    // re-read as a flag, and one containing a space would split. Its own
    // argv entry cannot do either.
    let id = "9f1c0b2e-0000-4000-8000-0123456789ab";
    for adapter in all() {
        let Some(invocation) = adapter.resume(id) else {
            continue;
        };
        let args: Vec<String> = invocation
            .args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.iter().any(|a| a == id),
            "{} does not pass the identifier as its own argument: {args:?}",
            adapter.id().slug()
        );
        assert_eq!(
            args.last().map(String::as_str),
            Some(id),
            "{} puts something after the identifier",
            adapter.id().slug()
        );
    }
}

#[test]
fn every_supported_harness_can_be_resumed() {
    // All seven document a resume mechanism in their own `--help`. This is
    // what makes Phase 7's resume work possible at all, so losing one
    // should be loud.
    for adapter in all() {
        assert!(
            adapter.resume("some-id").is_some(),
            "{} lost its resume mechanism",
            adapter.id().slug()
        );
    }
}

/// Each harness's resume shape, exactly as its installed binary documents
/// it. These four differ from one another in ways that matter — a flag, a
/// subcommand, a differently-spelled flag — which is the whole reason the
/// adapter layer exists.
#[test]
fn resume_shapes_match_the_installed_binaries() {
    let cases: [(IntegrationId, &[&str]); 7] = [
        (IntegrationId::ClaudeCode, &["--resume", "ID"]),
        (IntegrationId::Codex, &["resume", "ID"]),
        (IntegrationId::Antigravity, &["--conversation", "ID"]),
        (IntegrationId::OpenCode, &["--session", "ID"]),
        (IntegrationId::Cursor, &["--resume", "ID"]),
        (IntegrationId::Pi, &["--session", "ID"]),
        (IntegrationId::Hermes, &["--resume", "ID"]),
    ];
    for (id, expected) in cases {
        let adapter = adapter_for(id).expect("a harness");
        let invocation = adapter.resume("ID").expect("a resume mechanism");
        let args: Vec<String> = invocation
            .args()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, expected, "{} resumes differently", id.slug());
    }
}

#[test]
fn the_executable_names_match_the_installed_binaries() {
    // `agy` in particular: the Antigravity CLI's published package links
    // its binary onto PATH under that name, and Glasshouse searched only
    // for `antigravity` until a real install proved otherwise.
    assert_eq!(
        adapter_for(IntegrationId::Antigravity)
            .expect("a harness")
            .executable_candidates(),
        &["agy", "antigravity"]
    );
    assert_eq!(
        adapter_for(IntegrationId::Cursor)
            .expect("a harness")
            .executable_candidates(),
        &["cursor-agent"]
    );
}

// --- assigned identifiers -------------------------------------------

#[test]
fn assignment_agrees_with_the_declaration() {
    // An adapter that hands out `--session-id` arguments while declaring
    // that its identifiers can only be discovered, or the reverse, is
    // telling two different stories about the same harness. Phase 7 acts
    // on the declaration and Phase 7 builds the arguments, so the two
    // disagreeing would strand a session with an identifier nothing
    // recorded.
    for adapter in all() {
        let declared_assigned = matches!(
            adapter.describe().session_ids.value(),
            Some(SessionIds::Assigned { .. })
        );
        let assigns = adapter.assign_session_id("some-id").is_some();
        assert_eq!(
            declared_assigned,
            assigns,
            "{} declares assigned={declared_assigned} but assigns={assigns}",
            adapter.id().slug()
        );
    }
}

#[test]
fn a_discoverable_adapter_declares_discoverable_session_ids() {
    // Deliberately one-directional, unlike `assignment_agrees_with_the_
    // declaration` above: `SessionIds::Discoverable` describes a fact
    // about the *harness* (it names its own sessions and keeps a record
    // of them somewhere), which can be true, and correctly declared,
    // before Glasshouse has implemented reading that record — Cursor,
    // Hermes, Pi and OpenCode all declare it today with no
    // `session_id_source` yet. The direction that must never happen is
    // the other one: a real, working `session_id_source` whose adapter
    // tells a different story about itself.
    //
    // Combined with `assignment_agrees_with_the_declaration`, this also
    // rules out an adapter claiming both mechanisms: `describe()` names
    // exactly one `SessionIds` variant, so an adapter implementing both
    // `session_id_source` and `assign_session_id` would have to satisfy
    // "declares Discoverable" here and "declares Assigned" there for the
    // same declaration, which is impossible.
    for adapter in all() {
        if adapter.session_id_source().is_none() {
            continue;
        }
        assert!(
            matches!(
                adapter.describe().session_ids.value(),
                Some(SessionIds::Discoverable { .. })
            ),
            "{} has a session_id_source but does not declare SessionIds::Discoverable",
            adapter.id().slug()
        );
    }
}

#[test]
fn claude_code_assigns_the_identifier_its_binary_demands() {
    let adapter = adapter_for(IntegrationId::ClaudeCode).expect("a harness");
    let invocation = adapter
        .assign_session_id("9f1c0b2e-0000-4000-8000-0123456789ab")
        .expect("Claude Code accepts an assigned identifier");
    let args: Vec<String> = invocation
        .args()
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        vec!["--session-id", "9f1c0b2e-0000-4000-8000-0123456789ab"]
    );
}

#[test]
fn a_harness_that_cannot_be_told_its_identifier_assigns_none() {
    // Codex, Antigravity, OpenCode, Cursor, Pi and Hermes all name their
    // own sessions. Pretending otherwise would put a flag on a command
    // line that the harness does not have.
    for adapter in all() {
        if adapter.id() == IntegrationId::ClaudeCode {
            continue;
        }
        assert!(
            adapter.assign_session_id("some-id").is_none(),
            "{} claims it can be told its own session identifier",
            adapter.id().slug()
        );
    }
}

// --- messaging and interrupting -------------------------------------

#[test]
fn a_message_is_the_text_and_then_a_carriage_return() {
    for adapter in all() {
        let message = adapter.message("run the tests");
        assert_eq!(
            message.bytes(),
            b"run the tests\r",
            "{}",
            adapter.id().slug()
        );
    }
}

#[test]
fn an_interrupt_is_the_terminal_interrupt_byte() {
    for adapter in all() {
        assert_eq!(
            adapter.interrupt().bytes(),
            &[0x03],
            "{}",
            adapter.id().slug()
        );
    }
}

// --- lifecycle hooks -------------------------------------------------

fn hook_command() -> HookCommand {
    HookCommand::new(
        "/opt/glass house/glasshouse",
        "0123456789abcdef0123456789abcdef",
        "/state/sessions/0123456789abcdef0123456789abcdef",
        "/work/project",
        "/state",
        "/config",
    )
}

#[test]
fn claude_code_and_codex_are_the_harnesses_with_a_verified_hook_installation() {
    // The others declare hooks or do not, but neither has a *verified* way
    // to install them for one session without editing the user's own
    // configuration — which Glasshouse will not do.
    for adapter in all() {
        let installed = adapter.hook_installation(&hook_command()).is_some();
        let expected = matches!(
            adapter.id(),
            IntegrationId::ClaudeCode | IntegrationId::Codex
        );
        assert_eq!(
            installed,
            expected,
            "{} disagrees about installing hooks",
            adapter.id().slug()
        );
    }
}

#[test]
fn claude_codes_installation_still_goes_to_glasshouse_owned_state() {
    // Codex gaining a project-local destination must not change where
    // Claude Code's own installation lands.
    let installation = adapter_for(IntegrationId::ClaudeCode)
        .expect("a harness")
        .hook_installation(&hook_command())
        .expect("an installation");
    assert_eq!(installation.destination, HookDestination::GlasshouseOwned);
}

#[test]
fn the_generated_settings_document_is_valid_json_in_the_verified_shape() {
    let installation = adapter_for(IntegrationId::ClaudeCode)
        .expect("a harness")
        .hook_installation(&hook_command())
        .expect("an installation");

    let parsed: serde_json::Value = serde_json::from_str(&installation.contents)
        .unwrap_or_else(|err| panic!("not valid JSON: {err}\n{}", installation.contents));

    let hooks = parsed
        .get("hooks")
        .and_then(|h| h.as_object())
        .expect("a hooks object");

    for event in installation.events {
        let entries = hooks
            .get(*event)
            .and_then(|e| e.as_array())
            .unwrap_or_else(|| panic!("no entry for {event}"));
        let inner = entries[0]
            .get("hooks")
            .and_then(|h| h.as_array())
            .expect("an inner hooks array");
        // The shape a real Claude Code settings document uses: a list of
        // entries, each holding a list of {type, command, timeout}. None
        // of these is a tool event, so none carries a `matcher`.
        assert_eq!(inner[0]["type"], "command");
        assert!(inner[0]["timeout"].is_number());
        assert!(entries[0].get("matcher").is_none());

        let command = inner[0]["command"].as_str().expect("a command string");
        assert!(command.contains("hook"), "{command}");
        assert!(command.contains(&format!("--event {event}")), "{command}");
    }
}

#[test]
fn a_hook_command_pins_every_path_it_needs() {
    // A hook runs as a fresh process wherever the harness puts it. Left to
    // discover its own project it would report into the wrong one — which
    // is exactly what the first version did, exiting zero and updating
    // nothing.
    let command = hook_command().shell_command("Stop");
    for required in [
        "--scope",
        "--data-dir",
        "--config-dir",
        "--session",
        "--event",
    ] {
        assert!(command.contains(required), "{required} missing: {command}");
    }
}

#[test]
fn a_hook_command_survives_a_space_in_a_path() {
    let command = hook_command().shell_command("Stop");
    assert!(
        command.contains("'/opt/glass house/glasshouse'"),
        "an unquoted path with a space would run the wrong program: {command}"
    );
}

#[test]
fn a_generated_document_escapes_backslashes() {
    // A Windows executable path is full of them, and emitting them raw
    // would produce a document Claude Code cannot parse.
    let report = HookCommand::new(
        r"C:\Program Files\glasshouse.exe",
        "abcdef",
        r"C:\state",
        r"C:\project",
        r"C:\state",
        r"C:\config",
    );
    let installation = adapter_for(IntegrationId::ClaudeCode)
        .expect("a harness")
        .hook_installation(&report)
        .expect("an installation");
    let parsed: serde_json::Value = serde_json::from_str(&installation.contents)
        .unwrap_or_else(|err| panic!("not valid JSON: {err}\n{}", installation.contents));
    let command = parsed["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .expect("a command");
    assert!(
        command.contains(r"C:\Program Files\glasshouse.exe"),
        "the path did not survive a JSON round trip: {command}"
    );
}

#[test]
fn session_start_is_not_among_the_reported_events() {
    // Claude Code 2.1.245 does not fire it. A settings document declaring
    // one was installed and the hook never ran, while `UserPromptSubmit`
    // from the same document did. Adding it back would be a hook that
    // silently never reports.
    let installation = adapter_for(IntegrationId::ClaudeCode)
        .expect("a harness")
        .hook_installation(&hook_command())
        .expect("an installation");
    assert!(
        !installation.events.contains(&"SessionStart"),
        "SessionStart does not fire in this version"
    );
}

// --- the architecture the map fixes ---------------------------------

/// Crate names that would mean Glasshouse had reached inside a harness
/// instead of talking to its command line.
const HARNESS_INTERNALS: [&str; 5] = [
    "codex-core",
    "codex-tui",
    "codex-protocol",
    "claude-code",
    "cursor-agent",
];

/// Whether a manifest's dependency section names a harness's internals.
fn depends_on_harness_internals(manifest: &str) -> Option<&'static str> {
    let dependencies = manifest.split("[dependencies]").nth(1).unwrap_or(manifest);
    HARNESS_INTERNALS
        .into_iter()
        .find(|forbidden| dependencies.contains(forbidden))
}

/// "Avoid coupling Glasshouse core logic to Codex-internal Rust crates."
///
/// Codex is written in Rust, which makes depending on its internals
/// tempting in a way that Claude Code's TypeScript never could be. It
/// would also be a trap: Glasshouse would be pinned to one harness's
/// release cadence and internal types, for a harness it is supposed to
/// reach only through its command line like any other.
#[test]
fn glasshouse_depends_on_no_harness_internal_crate() {
    assert_eq!(
        depends_on_harness_internals(include_str!("../../Cargo.toml")),
        None
    );
}

/// The guard above is only worth having if it can fail. Checked against a
/// fabricated manifest rather than by editing the real one, because adding
/// a dependency that does not exist fails in cargo's resolver and proves
/// nothing about the test.
#[test]
fn the_dependency_guard_would_catch_a_coupling() {
    let manifest = "[package]\nname = \"glasshouse\"\n\n\
                    [dependencies]\nratatui = \"0.30\"\ncodex-core = \"0.1\"\n";
    assert_eq!(depends_on_harness_internals(manifest), Some("codex-core"));
    // A harness *named* in a comment or elsewhere is not a dependency.
    let innocent = "[package]\nname = \"glasshouse\"\n# codex-core is deliberately absent\n\
                    [dependencies]\nratatui = \"0.30\"\n";
    assert_eq!(depends_on_harness_internals(innocent), None);
}

/// "Make the generic PTY runtime independent from any specific harness
/// adapter."
#[test]
fn the_generic_pty_runtime_depends_on_no_adapter() {
    let modules = [
        ("pty/mod.rs", include_str!("../pty/mod.rs")),
        ("pty/process.rs", include_str!("../pty/process.rs")),
        ("session/runtime.rs", include_str!("../session/runtime.rs")),
    ];
    for (name, source) in modules {
        let code = production_code(source);
        for forbidden in ["HarnessAdapter", "crate::harness", "IntegrationId"] {
            assert!(
                !code.contains(forbidden),
                "{name} names `{forbidden}` in production code: the generic runtime has \
                 become dependent on a harness adapter"
            );
        }
    }
}

/// Phase 9A: "Never modify the user's normal global Claude Code or Codex
/// configuration merely to launch a Glasshouse profile."
///
/// Resolution turns a declaration into arguments, environment and
/// *described* documents for one child process. It has no business
/// touching the filesystem or the ambient environment at all — and a
/// module that never opens a file cannot modify a user's global harness
/// configuration. That is a stronger guarantee than enumerating the paths
/// it must avoid, and a much cheaper one to keep true.
///
/// **Line 362 gave Glasshouse a reason to write a file, and this test did
/// not get weaker for it.** `profile/generated.rs` is the one place that
/// writes, it is checked separately below, and the module that decides
/// *what* a launch is still opens nothing.
#[test]
fn resolving_a_launch_profile_touches_no_files() {
    let code = production_code(include_str!("../profile/mod.rs"));
    for forbidden in ["std::fs", "fs::", "File::", "OpenOptions", "std::env"] {
        assert!(
            !code.contains(forbidden),
            "profile/mod.rs names `{forbidden}` in production code: resolving a launch \
             profile must not touch the filesystem or the ambient environment, because \
             that is what keeps it structurally unable to modify the user's global \
             harness configuration"
        );
    }
}

/// Phase 9A lines 362 and 366: an isolated *generated* configuration
/// file, never an edit to a third-party one.
///
/// The one module in `profile/` that opens a file may not decide **which**
/// file. It is handed paths that came from
/// [`GeneratedConfigSite::file`] — the only thing allowed to say where a
/// generated document may live — so it must name no directory of its own
/// and must not read the ambient environment to find one.
///
/// The forbidden list is what a module would have to name in order to
/// arrive at somebody else's configuration: the environment (`HOME`,
/// `CODEX_HOME`, `APPDATA` are all read that way), this crate's own
/// directory resolver, and the two dot-directories a harness keeps its
/// configuration in.
#[test]
fn the_only_writer_in_profile_takes_its_paths_from_its_caller() {
    let code = production_code(include_str!("../profile/generated.rs"));
    for forbidden in [
        "std::env",
        "env::",
        "home_dir",
        "dirs::",
        "RuntimePaths",
        "crate::paths",
        ".claude",
        ".codex",
        ".config",
    ] {
        assert!(
            !code.contains(forbidden),
            "profile/generated.rs names `{forbidden}` in production code: the one writer \
             in this module must take every path from its caller, so that a generated \
             configuration can only ever land where `GeneratedConfigSite` said"
        );
    }
    // And it really is the only one: nothing else under `profile/` opens
    // a file, so a second writer cannot appear without failing a test.
    for (name, source) in [
        ("profile/mod.rs", include_str!("../profile/mod.rs")),
        (
            "profile/response.rs",
            include_str!("../profile/response.rs"),
        ),
    ] {
        let code = production_code(source);
        assert!(
            !code.contains("OpenOptions") && !code.contains("std::fs::write"),
            "{name} has become a second writer in `profile/`"
        );
    }
}

/// "Keep adapter-specific parsing isolated from the core Glasshouse
/// session model."
#[test]
fn the_session_model_depends_on_no_adapter() {
    let code = session_store_source();
    for forbidden in ["HarnessAdapter", "crate::harness", "IntegrationId"] {
        assert!(
            !code.contains(forbidden),
            "session/store names `{forbidden}` in production code: the session model \
             has become dependent on a harness adapter"
        );
    }
}

/// "Keep adapter-specific parsing isolated from the core Glasshouse
/// session model" cuts both ways: `session::native_id` depending on
/// `crate::harness` is fine and matches `session::select` (`discover`
/// takes a `&dyn HarnessAdapter`), but an adapter depending back on
/// `crate::session` is the same dependency pointed the wrong way — it
/// would make the two modules a cycle instead of the one-directional
/// relationship every other boundary test in this file enforces.
#[test]
fn no_adapter_depends_on_the_session_model() {
    let modules = [
        ("harness/antigravity.rs", include_str!("antigravity.rs")),
        ("harness/claude_code.rs", include_str!("claude_code.rs")),
        ("harness/codex.rs", include_str!("codex.rs")),
        ("harness/cursor.rs", include_str!("cursor.rs")),
        ("harness/hermes.rs", include_str!("hermes.rs")),
        ("harness/opencode.rs", include_str!("opencode.rs")),
        ("harness/pi.rs", include_str!("pi.rs")),
    ];
    for (name, source) in modules {
        let code = production_code(source);
        assert!(
            !code.contains("crate::session"),
            "{name} names `crate::session` in production code: an adapter has become \
             dependent on the session model it is supposed to be described *by*, not \
             coupled to"
        );
    }
}

/// The scan above is only worth having if it can fail.
#[test]
fn the_adapter_dependency_scan_would_catch_a_violation() {
    let violating = "use crate::session::native_id;\nfn read() {}";
    assert!(production_code(violating).contains("crate::session"));
    // ... and does not fire on a doc comment that merely mentions the
    // module, the same way `harness/mod.rs`'s own doc comments legitimately
    // do (e.g. mentioning `crate::session::select`).
    let documented = "/// See [`mod@crate::session::native_id`].\nfn read() {}";
    assert!(!production_code(documented).contains("crate::session"));
}

/// The scan above is only worth having if it can fail.
#[test]
fn the_dependency_scan_would_catch_a_violation() {
    let violating = "fn spawn() {\n    if id == IntegrationId::ClaudeCode { todo!() }\n}";
    assert!(production_code(violating).contains("IntegrationId"));
    // ... and does not fire on a doc comment that merely mentions one.
    let documented = "/// Holds an [`IntegrationId`] as a string.\nfn spawn() {}";
    assert!(!production_code(documented).contains("IntegrationId"));
    // ... nor on a test.
    let tested = "fn spawn() {}\n#[cfg(test)]\nmod tests { use IntegrationId; }";
    assert!(!production_code(tested).contains("IntegrationId"));
}

/// Map line 2404: a harness named as having a structured pre-tool hook must
/// actually declare that event, or `glasshouse doctor` would promise
/// coordination the harness never offered.
#[test]
fn a_named_pre_tool_hook_is_one_the_adapter_itself_declares() {
    for adapter in super::all() {
        let Some(event) = super::structured_pre_tool_hook(adapter.id()) else {
            continue;
        };
        let hooks = adapter
            .describe()
            .hooks
            .value()
            .copied()
            .unwrap_or_else(|| {
                panic!(
                    "{} names {event} but declares no hooks",
                    adapter.id().slug()
                )
            });
        assert!(
            hooks.verified_events.contains(&event),
            "{} names {event} as its pre-tool hook and does not declare it",
            adapter.id().slug()
        );
    }
}

/// The other direction, and the honest half: today exactly one harness has a
/// bridge, and every other one answers `None` rather than a guess.
#[test]
fn claude_code_is_the_only_harness_with_a_verified_pre_tool_bridge() {
    let named: Vec<_> = super::all()
        .filter(|adapter| super::structured_pre_tool_hook(adapter.id()).is_some())
        .map(|adapter| adapter.id())
        .collect();
    assert_eq!(named, vec![crate::integrations::IntegrationId::ClaudeCode]);
}
