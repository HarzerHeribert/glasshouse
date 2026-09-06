use pane::contract::{Conversation, Message, Role, ServedBy};
use pane::runtime::handles::HandleTable;
use pane::tui::{
    Activity, CellError, CellView, Notebook, ScreenState, SidebarVisibility, SupervisorStatus,
    render_screen, screen_regions, slash_matches,
};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, layout::Rect};

fn state() -> ScreenState {
    ScreenState {
        model: Some("claude-fable-5.1".into()),
        project: Some("glasshouse".into()),
        sandbox: Some("3p/4c".into()),
        network: Some("off".into()),
        connected: Some(true),
        ..ScreenState::default()
    }
}
fn conversation() -> Conversation {
    Conversation {
        system: String::new(),
        messages: vec![
            Message::text(Role::User, "Inspect the request path."),
            Message::text(Role::Assistant, "I found the gateway configuration."),
        ],
    }
}
fn draw(
    width: u16,
    height: u16,
    state: &ScreenState,
    conversation: &Conversation,
    notebook: &Notebook,
) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            render_screen(
                frame,
                conversation,
                &ServedBy::default(),
                &HandleTable::new(),
                notebook,
                state,
            )
        })
        .unwrap();
    terminal.backend().buffer().clone()
}
fn text(buffer: &Buffer) -> String {
    (buffer.area.y..buffer.area.bottom())
        .map(|y| {
            (buffer.area.x..buffer.area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom()
}

#[test]
fn a_turn_block_has_a_visible_boundary_and_a_header() {
    let buffer = draw(80, 30, &state(), &conversation(), &Notebook::default());
    let rendered = text(&buffer);
    for (header, body) in [("USER", "Inspect"), (" PANE", "I found")] {
        let lines: Vec<_> = rendered.lines().collect();
        let at = lines
            .iter()
            .position(|line| line.contains(header) && !line.contains("PANE /"))
            .unwrap();
        assert_eq!(buffer[(0, at as u16)].bg, ratatui::style::Color::Reset);
        assert!(lines[at + 1].contains(body));
        assert!(lines[at + 1].starts_with(' '));
    }
}

#[test]
fn no_region_writes_outside_its_own_rect_at_60_80_120_and_200_columns() {
    for width in [60, 80, 120, 200] {
        let mut state = state();
        state.input = "Ж".repeat(300);
        state.model = Some("Ф".repeat(100));
        state.project = Some("Ю".repeat(100));
        let conversation = Conversation {
            system: String::new(),
            messages: vec![
                Message::text(Role::User, "Щ".repeat(100)),
                Message::text(Role::Assistant, "Ц".repeat(100)),
            ],
        };
        let notebook = Notebook {
            supervisor: Some(SupervisorStatus::Nudged("Э".repeat(200))),
            cells: vec![CellView {
                table: Some("Б".repeat(100)),
                error: Some(CellError {
                    class: "Error".into(),
                    message: "Д".repeat(100),
                    ..CellError::default()
                }),
                ..CellView::default()
            }],
            ..Notebook::default()
        };
        let buffer = draw(width, 70, &state, &conversation, &notebook);
        let r = screen_regions(buffer.area, &state);
        let rects = [
            r.header,
            r.transcript,
            r.details,
            r.completions,
            r.input,
            r.status,
        ];
        for y in 0..buffer.area.height {
            for x in 0..width {
                assert!(rects.iter().filter(|rect| contains(**rect, x, y)).count() <= 1);
                for glyph in buffer[(x, y)].symbol().chars() {
                    let valid = match glyph {
                        'Ж' => contains(r.input, x, y),
                        'Ф' => contains(r.status, x, y),
                        'Ю' => contains(r.header, x, y) || contains(r.status, x, y),
                        'Щ' | 'Ц' | 'Б' | 'Д' => contains(r.transcript, x, y),
                        'Э' => contains(r.details, x, y),
                        _ => true,
                    };
                    assert!(valid, "{glyph} escaped at {width}: {x},{y}");
                }
            }
        }
        let rendered = text(&buffer);
        for glyph in ['Ж', 'Ф', 'Ю', 'Щ', 'Ц', 'Б', 'Д'] {
            assert!(rendered.contains(glyph), "missing {glyph} at {width}");
        }
        // Every body starts intact after the rail, including the first column.
        for glyph in ['Щ', 'Ц', 'Б'] {
            assert!(
                rendered
                    .lines()
                    .any(|line| line.starts_with(&format!(" {glyph}")))
            );
        }
        let at: Vec<_> = ['Щ', 'Ц', 'Б', 'Д']
            .into_iter()
            .map(|c| rendered.find(c).unwrap())
            .collect();
        assert!(
            at.windows(2).all(|pair| pair[0] < pair[1]),
            "turns interleaved"
        );
    }
}

#[test]
fn the_status_line_names_the_model_the_project_the_sandbox_and_the_connection() {
    for width in [60, 80, 120, 200] {
        let rendered = text(&draw(
            width,
            24,
            &state(),
            &conversation(),
            &Notebook::default(),
        ));
        for word in [
            "claude-fable-5.1",
            "glasshouse",
            "sandbox 3p/4c",
            "net:off",
            "Glasshouse connected",
        ] {
            assert!(rendered.contains(word), "{word}: {rendered}");
        }
    }
}
#[test]
fn the_input_area_shows_what_is_being_composed_and_is_separated_from_the_transcript() {
    let mut state = state();
    state.input = "first line\nsecond line".into();
    let buffer = draw(60, 24, &state, &conversation(), &Notebook::default());
    let r = screen_regions(buffer.area, &state);
    assert!(text(&buffer).contains("first line"));
    assert!(text(&buffer).contains("second line"));
    let rendered = text(&buffer);
    let first = rendered
        .lines()
        .position(|line| line.contains("first line"))
        .unwrap();
    let second = rendered
        .lines()
        .position(|line| line.contains("second line"))
        .unwrap();
    assert_eq!(second, first + 1);
    assert!(
        text(&buffer)
            .lines()
            .nth(usize::from(r.input.y))
            .unwrap()
            .contains("───")
    );
    assert!(r.transcript.bottom() <= r.input.y && r.input.bottom() <= r.status.y);
}
#[test]
fn slash_completion_uses_real_commands_and_filters_as_letters_arrive() {
    assert_eq!(slash_matches("/").len(), 20);
    assert_eq!(
        slash_matches("/mo"),
        vec![
            ("/model".into(), "select the active model"),
            ("/motion".into(), "on or off · reduce animation"),
            ("/mode".into(), "execute or plan without running code")
        ]
    );
    for input in ["hello", "/model something", "/unknown"] {
        assert!(slash_matches(input).is_empty());
    }
    let mut state = state();
    state.input = "/mo".into();
    let buffer = draw(60, 24, &state, &conversation(), &Notebook::default());
    let r = screen_regions(buffer.area, &state);
    let rendered = text(&buffer);
    assert!(
        rendered
            .lines()
            .nth(usize::from(r.completions.y))
            .unwrap()
            .contains("/model")
    );
    assert!(rendered.contains("select the active model"));
}
#[test]
fn the_root_view_does_not_draw_an_outer_window_border() {
    for width in [60, 80, 120, 200] {
        let buffer = draw(width, 24, &state(), &conversation(), &Notebook::default());
        assert_ne!(buffer[(0, 0)].symbol(), "┌");
        assert_ne!(buffer[(width - 1, 23)].symbol(), "┘");
        assert!(!text(&buffer).contains("Conversation"));
    }
}
#[test]
fn narrow_layout_collapses_secondary_regions_instead_of_overlapping_them() {
    for width in [60, 80] {
        assert_eq!(
            screen_regions(Rect::new(0, 0, width, 24), &state())
                .details
                .width,
            0
        );
    }
    for width in [120, 200] {
        assert_eq!(
            screen_regions(Rect::new(0, 0, width, 24), &state())
                .transcript
                .width,
            width - 36
        );
    }
}
#[test]
fn wide_telemetry_preserves_reported_fields_and_budget_provenance() {
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    let served = ServedBy {
        provider: Some("anthropic".into()),
        model: Some("claude-fable-5.1".into()),
        route: Some("direct".into()),
        quota_context: Some("subscription".into()),
        input_tokens: Some(123),
        output_tokens: Some(456),
        cached_input_tokens: Some(100),
    };
    let notebook = Notebook {
        tokens: Some(pane::tui::TaskTokens {
            used: 579,
            cap: 400_000,
            counted: pane::tui::Counted::Gateway,
        }),
        supervisor: Some(SupervisorStatus::Nudged("check the request".into())),
        ..Notebook::default()
    };
    terminal
        .draw(|frame| {
            render_screen(
                frame,
                &conversation(),
                &served,
                &HandleTable::new(),
                &notebook,
                &state(),
            )
        })
        .unwrap();
    let rendered = text(terminal.backend().buffer());
    for field in [
        "anthropic",
        "subscription",
        "route: direct",
        "cached input: 100",
        "tokens: 123 in / 456 out",
        "budget: 579/400000 tok",
        "counted: reported",
        "supervisor: check the request",
    ] {
        assert!(rendered.contains(field), "{field}: {rendered}");
    }
}

#[test]
fn the_sidebar_can_be_hidden_and_the_preference_survives_resize() {
    let mut state = state();
    let notebook = Notebook {
        supervisor: Some(SupervisorStatus::Nudged("TELEMETRY_SENTINEL".into())),
        ..Notebook::default()
    };
    assert!(
        text(&draw(120, 24, &state, &conversation(), &notebook)).contains("TELEMETRY_SENTINEL")
    );
    state.sidebar = SidebarVisibility::Hidden;
    for width in [200, 60, 120] {
        let buffer = draw(width, 24, &state, &conversation(), &notebook);
        assert_eq!(screen_regions(buffer.area, &state).details.width, 0);
        assert!(!text(&buffer).contains("TELEMETRY_SENTINEL"));
        assert!(text(&buffer).contains("Glasshouse connected"));
    }
    state.sidebar = SidebarVisibility::Auto;
    assert!(
        text(&draw(120, 24, &state, &conversation(), &notebook)).contains("TELEMETRY_SENTINEL")
    );
}

#[test]
fn sidebar_width_is_fixed_and_explicit_visibility_works_at_80_columns() {
    let mut state = state();
    for width in [120, 200, 300] {
        let r = screen_regions(Rect::new(5, 3, width, 30), &state);
        assert_eq!(r.details.width, 34);
        assert_eq!(r.details.right(), 5 + width);
        assert!(r.details.x >= r.transcript.right() + 2);
    }
    assert_eq!(
        screen_regions(Rect::new(0, 0, 80, 30), &state)
            .details
            .width,
        0
    );
    state.sidebar = SidebarVisibility::Shown;
    assert_eq!(
        screen_regions(Rect::new(0, 0, 80, 30), &state)
            .details
            .width,
        34
    );
    assert_eq!(
        screen_regions(Rect::new(0, 0, 60, 30), &state)
            .details
            .width,
        0
    );
}

#[test]
fn animated_indicator_frames_have_constant_bounds_and_history_is_stable() {
    for activity in [
        Activity::Starting,
        Activity::Thinking,
        Activity::Streaming,
        Activity::Executing,
        Activity::Searching,
        Activity::Waiting,
        Activity::Compacting,
        Activity::Complete,
        Activity::Failed,
    ] {
        let mut state = state();
        state.activity = activity;
        let first = draw(60, 24, &state, &conversation(), &Notebook::default());
        for tick in 0..12 {
            assert_eq!(activity.indicator(tick).len(), 4);
            state.animation_frame = tick;
            let next = draw(60, 24, &state, &conversation(), &Notebook::default());
            let r = screen_regions(next.area, &state);
            for y in r.transcript.y..r.transcript.bottom() {
                for x in 0..60 {
                    assert_eq!(first[(x, y)], next[(x, y)]);
                }
            }
            if [Activity::Complete, Activity::Failed].contains(&activity) {
                assert_eq!(first, next);
            }
        }
    }
}
#[test]
fn a_fresh_terminal_over_existing_output_erases_every_old_cell() {
    use ratatui::backend::Backend;
    use ratatui::buffer::Cell;
    for width in [60, 80, 120, 200] {
        let mut backend = TestBackend::new(width, 30);
        let old_cell = Cell::new("Z");
        let old = &old_cell;
        backend
            .draw((0..30).flat_map(|y| (0..width).map(move |x| (x, y, old))))
            .unwrap();
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_screen(
                    frame,
                    &conversation(),
                    &ServedBy::default(),
                    &HandleTable::new(),
                    &Notebook::default(),
                    &state(),
                )
            })
            .unwrap();
        assert_eq!(
            terminal.backend().buffer(),
            &draw(width, 30, &state(), &conversation(), &Notebook::default()),
            "old stdout survived a new Terminal at {width}"
        );
    }
}

#[test]
fn resizing_clears_stale_cells_and_tiny_terminals_do_not_panic() {
    let mut terminal = Terminal::new(TestBackend::new(200, 30)).unwrap();
    let mut state = state();
    state.input = "/".into();
    for (width, height) in [
        (200, 30),
        (60, 24),
        (120, 30),
        (80, 24),
        (1, 1),
        (2, 3),
        (0, 0),
    ] {
        terminal.backend_mut().resize(width, height);
        terminal.autoresize().unwrap();
        terminal
            .draw(|frame| {
                render_screen(
                    frame,
                    &conversation(),
                    &ServedBy::default(),
                    &HandleTable::new(),
                    &Notebook::default(),
                    &state,
                )
            })
            .unwrap();
        assert_eq!(
            terminal.backend().buffer(),
            &draw(width, height, &state, &conversation(), &Notebook::default())
        );
    }
}
#[test]
fn long_unicode_transcripts_follow_the_tail_without_losing_the_newest_turn() {
    let mut conversation = conversation();
    conversation
        .messages
        .insert(0, Message::text(Role::User, "界 e\u{301} ".repeat(2000)));
    conversation
        .messages
        .push(Message::text(Role::User, "LAST TURN INTACT"));
    let rendered = text(&draw(60, 24, &state(), &conversation, &Notebook::default()));
    assert!(rendered.contains("LAST TURN INTACT"));
}
#[test]
fn visual_review_captures() {
    let Some(dir) = std::env::var_os("PANE_LOOK_CAPTURES") else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    for width in [60, 80, 120, 200] {
        for activity in [
            Activity::Starting,
            Activity::Thinking,
            Activity::Streaming,
            Activity::Executing,
            Activity::Complete,
            Activity::Failed,
        ] {
            let mut state = state();
            state.activity = activity;
            state.animation_frame = 2;
            state.input = if activity == Activity::Complete {
                "/mo"
            } else {
                "explain the request failure"
            }
            .into();
            let notebook = Notebook {
                cells: vec![CellView {
                    table: Some(
                        "config  File  gateway.toml · 42 lines\n  model = requested_model".into(),
                    ),
                    error: if activity == Activity::Failed {
                        Some(CellError {
                            class: "RequestError".into(),
                            message: "request failed: model unavailable".into(),
                            line: Some(3),
                            column: Some(1),
                        })
                    } else {
                        None
                    },
                    returned: if activity == Activity::Complete {
                        Some("The request uses an unavailable model.".into())
                    } else {
                        None
                    },
                    ..CellView::default()
                }],
                ..Notebook::default()
            };
            let conversation = if activity == Activity::Starting {
                Conversation {
                    system: String::new(),
                    messages: vec![],
                }
            } else {
                conversation()
            };
            let buffer = draw(width, 30, &state, &conversation, &notebook);
            std::fs::write(
                std::path::Path::new(&dir).join(format!("{width}-{}.txt", activity.label())),
                text(&buffer),
            )
            .unwrap();
        }
    }
}

#[test]
fn natural_narration_and_explicit_output_survive_while_long_details_fold_locally() {
    let source = (0..24)
        .map(|i| format!("const item_{i} = {i};"))
        .collect::<Vec<_>>()
        .join("\n");
    let conversation = Conversation {
        system: String::new(),
        messages: vec![
            Message::text(Role::User, "inspect the file"),
            Message::text(
                Role::Assistant,
                format!("I will inspect it.\n\n```pane\n{source}\n```\nThe result follows."),
            ),
            Message::text(Role::Assistant, "USER ANSWER"),
        ],
    };
    let notebook = Notebook {
        cells: vec![CellView {
            table: Some(
                (0..20)
                    .map(|i| format!("preview_{i}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            stdout: Some("EXPLICIT OUTPUT".into()),
            returned: Some("USER ANSWER".into()),
            ..CellView::default()
        }],
        ..Notebook::default()
    };
    let mut state = state();
    state.compact = true;
    let folded = text(&draw(120, 90, &state, &conversation, &notebook));
    for text in [
        "I will inspect it.",
        "The result follows.",
        "EXPLICIT OUTPUT",
        "USER ANSWER",
        "Ctrl-O · code and results",
    ] {
        assert!(folded.contains(text));
    }
    assert_eq!(folded.matches("USER ANSWER").count(), 1);
    assert!(!folded.contains("const item_23"));
    assert!(!folded.contains("preview_19"));
    state.compact = false;
    let expanded = text(&draw(120, 90, &state, &conversation, &notebook));
    assert!(expanded.contains("const item_23 = 23;"));
    assert!(expanded.contains("preview_19"));
}

#[test]
fn transparent_themes_keep_blocks_separate_and_preserve_text() {
    use pane::tui::Theme;
    use ratatui::style::Color;
    let mut state = state();
    state.theme = Theme::Amber;
    let amber = draw(120, 40, &state, &conversation(), &Notebook::default());
    assert!(amber.content.iter().any(|cell| cell.bg == Color::Reset));
    let regions = screen_regions(amber.area, &state);
    for y in regions.transcript.y..regions.transcript.bottom() {
        for x in regions.transcript.x..regions.transcript.right() {
            assert_eq!(amber[(x, y)].bg, Color::Reset);
        }
    }
    assert_ne!(
        amber[(regions.input.x, regions.input.y + 1)].bg,
        Color::Reset
    );
    assert!(
        amber
            .content
            .iter()
            .any(|cell| cell.fg == Color::LightYellow)
    );
    let rendered = text(&amber);
    assert!(rendered.contains("USER"));
    assert!(!rendered.contains("╭─") && !rendered.contains("╰─"));
    assert!(
        (regions.transcript.y..regions.transcript.bottom()).any(|y| {
            (regions.transcript.x..regions.transcript.right())
                .all(|x| amber[(x, y)].symbol().trim().is_empty())
        })
    );
    state.theme = Theme::Ice;
    let ice = draw(120, 40, &state, &conversation(), &Notebook::default());
    assert_eq!(text(&ice), rendered);
    assert!(ice.content.iter().any(|cell| cell.fg == Color::LightCyan));
}

#[test]
fn partial_responses_stay_in_the_active_block_and_only_the_indicator_moves() {
    let mut state = state();
    state.activity = Activity::Streaming;
    state.streaming_text = Some("A partial response with an unfinished ```pane fence".into());
    let first = draw(80, 40, &state, &conversation(), &Notebook::default());
    state.animation_frame += 1;
    let next = draw(80, 40, &state, &conversation(), &Notebook::default());
    assert!(text(&first).contains("PANE / RECEIVING"));
    assert!(text(&first).contains("unfinished ```pane fence"));
    let region = screen_regions(first.area, &state).transcript;
    assert_ne!(first, next);
    for y in region.y..region.bottom() {
        let row: String = (region.x..region.right())
            .map(|x| first[(x, y)].symbol())
            .collect();
        if !row.contains("PANE / RECEIVING") {
            for x in region.x..region.right() {
                assert_eq!(first[(x, y)], next[(x, y)]);
            }
        }
    }
    state.streaming_text = None;
    state.activity = Activity::Complete;
    let done = draw(80, 40, &state, &conversation(), &Notebook::default());
    assert!(!text(&done).contains("PANE / RECEIVING"));
}

#[test]
fn composer_grows_with_text_and_statusline_preferences_preserve_bounds() {
    use pane::tui::StatusLine;
    let mut state = state();
    let small = screen_regions(ratatui::layout::Rect::new(0, 0, 80, 40), &state);
    state.input = "line\n".repeat(8);
    let large = screen_regions(ratatui::layout::Rect::new(0, 0, 80, 40), &state);
    assert!(large.input.height > small.input.height);
    assert!(large.transcript.bottom() <= large.input.y);
    for (setting, height) in [(StatusLine::Compact, 1), (StatusLine::Hidden, 0)] {
        state.status_line = setting;
        let regions = screen_regions(ratatui::layout::Rect::new(0, 0, 80, 40), &state);
        assert_eq!(regions.status.height, height);
        assert_eq!(regions.input.bottom(), regions.status.y);
    }
}

#[test]
fn code_is_formatted_locally_and_original_source_is_available_expanded() {
    let source = "const result={ok:true,items:[1,2]};if(result.ok){console.log(result);}";
    let conversation = Conversation {
        system: String::new(),
        messages: vec![
            Message::text(Role::User, "Inspect."),
            Message::text(Role::Assistant, format!("```pane\n{source}\n```")),
        ],
    };
    let mut state = state();
    state.pretty = true;
    let pretty = text(&draw(200, 40, &state, &conversation, &Notebook::default()));
    assert!(pretty.contains("const result ="), "{pretty}");
    assert!(pretty.contains("  console.log(result);"), "{pretty}");
    state.pretty = false;
    let raw = text(&draw(200, 40, &state, &conversation, &Notebook::default()));
    assert!(raw.contains(source));
    let pane::contract::Block::Text(stored) = &conversation.messages[1].content[0];
    assert!(stored.contains(source));
}

#[test]
fn transparent_sections_use_the_available_transcript_width() {
    for width in [60, 80, 120, 200, 300] {
        for sidebar in [SidebarVisibility::Auto, SidebarVisibility::Hidden] {
            let mut state = state();
            state.sidebar = sidebar;
            let buffer = draw(width, 30, &state, &conversation(), &Notebook::default());
            let regions = screen_regions(buffer.area, &state);
            let expected = if regions.details.width > 0 {
                width - 36
            } else {
                width
            };
            assert_eq!(regions.transcript.width, expected);
            let y = regions.transcript.y;
            assert_eq!(
                buffer[(regions.transcript.right() - 1, y)].bg,
                ratatui::style::Color::Reset
            );
        }
    }
}

#[test]
fn compact_view_hides_protocol_noise_and_keeps_actual_results() {
    let mut state = state();
    state.compact = true;
    let c = Conversation {
        system: String::new(),
        messages: vec![
            Message::text(Role::User, "Read roman.py"),
            Message::text(
                Role::Assistant,
                "```pane\nread({path: 'roman.py'});\n```\n```pane\nreturn 'guess';\n```",
            ),
        ],
    };
    let shown = text(&draw(120, 35, &state, &c, &Notebook::default()));
    assert!(shown.contains("Response format rejected"));
    assert!(shown.contains("nothing ran"));
    assert!(!shown.contains("```"));
    assert!(!shown.contains("return 'guess'"));
    assert!(!shown.contains("TOOL / PREVIEW"));
}

#[test]
fn speculative_calls_are_never_presented_as_observed_results() {
    let mut state = state();
    state.compact = true;
    let c = Conversation {
        system: String::new(),
        messages: vec![
            Message::text(Role::User, "Check the branch."),
            Message::text(
                Role::Assistant,
                "```pane\nif (false) { await bash({command: 'echo skipped'}); } return 'done';\n```",
            ),
        ],
    };
    let n = Notebook {
        cells: vec![CellView {
            execution: Some("No tool calls ran in this cell.".into()),
            returned: Some("done".into()),
            ..CellView::default()
        }],
        ..Notebook::default()
    };
    let shown = text(&draw(120, 35, &state, &c, &n));
    assert!(shown.contains("planned: bash"));
    assert!(shown.contains("No tools ran."));
    assert!(!shown.contains("bash · returned"));
    assert!(!shown.contains("inference turn"));
}

#[test]
fn completion_scan_is_bounded_and_does_not_change_history() {
    let mut state = state();
    state.activity = Activity::Complete;
    state.completion_tick = Some(0);
    let first = draw(120, 35, &state, &conversation(), &Notebook::default());
    state.completion_tick = Some(4);
    let next = draw(120, 35, &state, &conversation(), &Notebook::default());
    assert_ne!(first, next);
    let region = screen_regions(first.area, &state).transcript;
    for y in region.y..region.bottom() {
        for x in region.x..region.right() {
            assert_eq!(first[(x, y)], next[(x, y)]);
        }
    }
    state.completion_tick = None;
    let settled = text(&draw(
        120,
        35,
        &state,
        &conversation(),
        &Notebook::default(),
    ));
    assert!(settled.contains("complete"));
    assert!(!settled.contains("cell completed"));
}

#[test]
fn command_panels_erase_underlying_transcript_text() {
    let c = Conversation {
        system: String::new(),
        messages: vec![Message::text(Role::User, "STALE_TRANSCRIPT ".repeat(60))],
    };
    let mut state = state();
    state.panel = Some(pane::tui::Panel::text("Models", "one\ntwo"));
    let shown = text(&draw(200, 40, &state, &c, &Notebook::default()));
    assert!(shown.contains("Models"));
    assert!(!shown.contains("STALE_TRANSCRIPT"));
}

#[test]
fn local_file_diffs_are_visible_in_compact_and_expanded_views() {
    let c = Conversation {
        system: String::new(),
        messages: vec![
            Message::text(Role::User, "Edit the file"),
            Message::text(Role::Assistant, "```pane\nreturn 'done';\n```"),
        ],
    };
    let n = Notebook {
        cells: vec![CellView {
            changes: Some(
                "--- example.py\n+++ example.py\n@@ -1,1 +1,1 @@\n-old_value\n+new_value".into(),
            ),
            execution: Some("No tool calls ran in this cell.".into()),
            returned: Some("done".into()),
            ..CellView::default()
        }],
        ..Notebook::default()
    };
    for compact in [true, false] {
        let mut state = state();
        state.compact = compact;
        let shown = text(&draw(120, 40, &state, &c, &n));
        assert!(shown.contains("CHANGES OBSERVED"), "{shown}");
        assert!(shown.contains("-old_value"), "{shown}");
        assert!(shown.contains("+new_value"), "{shown}");
    }
}

#[test]
fn natural_answers_render_tables_and_never_invent_tool_sections() {
    let conversation = Conversation {
        system: String::new(),
        messages: vec![
            Message::text(Role::User, "Report your findings."),
            Message::text(
                Role::Assistant,
                "## Findings\n**Verified:** reads work.\n\n| Feature | Result |\n|---|---|\n| Read | Correct |\n| Write | Nested files work |",
            ),
        ],
    };
    for width in [60, 80, 120, 200] {
        for compact in [true, false] {
            let mut state = state();
            state.compact = compact;
            let shown = text(&draw(
                width,
                30,
                &state,
                &conversation,
                &Notebook::default(),
            ));
            assert!(shown.contains("Findings"));
            assert!(shown.contains("Nested files work"));
            assert!(!shown.contains("## Findings"));
            assert!(!shown.contains("**Verified:**"));
            assert!(!shown.contains("TOOL / PREVIEW"));
            assert!(!shown.contains("(no outputs)"));
        }
    }
}

#[test]
fn telemetry_distinguishes_proposals_measurements_and_actual_execution() {
    use pane::telemetry::RequestMeasurement;
    let c = Conversation {
        system: String::new(),
        messages: vec![
            Message::text(Role::User, "Inspect one file"),
            Message::text(
                Role::Assistant,
                "```pane\nawait read({path:'roman.py'}); if(false) await glob({pattern:'*'});\n```",
            ),
        ],
    };
    let n = Notebook {
        requests: vec![RequestMeasurement::from_response(
            1,
            "deepseek-v4-flash".into(),
            1200,
            ServedBy::default(),
            Some(&pane::wire::Usage {
                input_tokens: 100,
                output_tokens: 20,
            }),
        )],
        cells: vec![CellView {
            execution: Some("└─ read roman.py · returned".into()),
            ..CellView::default()
        }],
        ..Notebook::default()
    };
    for width in [60, 80, 120, 200] {
        let mut state = state();
        state.telemetry_open = true;
        let buffer = draw(width, 44, &state, &c, &n);
        let shown = text(&buffer);
        for expected in [
            "TELEMETRY",
            "REQUEST 01",
            "read roman.py",
            "cost unreported",
            "proposed",
            "observed",
        ] {
            assert!(shown.contains(expected), "{expected} at {width}: {shown}");
        }
        assert!(!shown.contains("glob · returned"));
        assert!(!shown.contains("tokens saved"));
        assert!(!shown.contains("$0"));
        let regions = screen_regions(buffer.area, &state);
        assert!(regions.transcript.bottom() <= regions.input.y);
    }
}

#[test]
fn telemetry_motion_is_local_and_reduced_motion_stays_still() {
    let c = conversation();
    let mut state = state();
    state.telemetry_open = true;
    state.activity = Activity::Thinking;
    let first = draw(120, 44, &state, &c, &Notebook::default());
    state.animation_frame = 14;
    let moved = draw(120, 44, &state, &c, &Notebook::default());
    assert_ne!(first, moved);
    state.reduced_motion = true;
    let reduced = draw(120, 44, &state, &c, &Notebook::default());
    state.animation_frame = 24;
    let later = draw(120, 44, &state, &c, &Notebook::default());
    // The telemetry body is still. The legacy global activity indicator has
    // its own clock, frozen by the live loop when reduced motion is selected.
    let area = screen_regions(reduced.area, &state).transcript;
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            assert_eq!(reduced[(x, y)], later[(x, y)]);
        }
    }
}

#[test]
fn delivery_trace_counts_real_bytes_and_has_bounded_history() {
    let mut pulse = pane::tui::Pulse::default();
    for _ in 0..50 {
        pulse.receive(7);
    }
    assert_eq!(pulse.bytes, 350);
    assert_eq!(pulse.deltas, 50);
    assert_eq!(pulse.deliveries.len(), 32);
}

#[test]
fn cell_repair_shows_the_amended_code_and_keeps_the_original_failure() {
    let c = Conversation {
        system: String::new(),
        messages: vec![
            Message::text(Role::User, "Answer briefly"),
            Message::text(Role::Assistant, "```pane\nreturn 'ok;\n```"),
            Message::text(Role::User, "SyntaxError; repair is available"),
            Message::text(
                Role::Assistant,
                "```pane-edit\n{\"cell\":1,\"replace\":\"'ok;\",\"with\":\"'ok';\"}\n```",
            ),
        ],
    };
    let n = Notebook {
        cells: vec![
            CellView {
                error: Some(CellError {
                    class: "SyntaxError".into(),
                    message: "Unterminated string".into(),
                    line: Some(1),
                    column: Some(7),
                }),
                answered: true,
                execution: Some("No tool calls ran in this cell.".into()),
                ..CellView::default()
            },
            CellView {
                executed_source: Some("return 'ok';".into()),
                repaired_from: Some(1),
                returned: Some("ok".into()),
                execution: Some("No tool calls ran in this cell.".into()),
                ..CellView::default()
            },
        ],
        ..Notebook::default()
    };
    for compact in [true, false] {
        let mut state = state();
        state.compact = compact;
        let shown = text(&draw(100, 44, &state, &c, &n));
        assert!(shown.contains("Unterminated string"), "{shown}");
        assert!(shown.contains("Amends syntax-failed cell 1"), "{shown}");
        assert!(!shown.contains("\"replace\""), "{shown}");
        if compact {
            assert!(shown.contains("Cell repaired"));
        } else {
            assert!(shown.contains("return 'ok';"));
        }
    }
}
