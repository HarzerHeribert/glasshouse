//! Real PTY + terminal-emulator coverage; no external model or credentials.
//!
//! **The harness answers a cursor-position query, and on Windows nothing works
//! without it.** A terminal that is asked `ESC[6n` replies with the cursor's
//! row and column; crossterm needs that answer on Windows, where there is no
//! ioctl to read it from, and blocks until it arrives. Measured on the ARM64
//! Windows VM: pane emitted exactly those four bytes and then waited forever,
//! so every test in this file timed out against a blank screen while pane
//! itself was perfectly healthy. Unix never showed it because crossterm reads
//! the position from the kernel there and never asks.
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

struct App {
    master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    input: Box<dyn Write + Send>,
    output: mpsc::Receiver<Vec<u8>>,
    screen: vt100::Parser,
    bytes: Vec<u8>,
    root: PathBuf,
    #[cfg(unix)]
    terminal_flags: Vec<u8>,
}
impl App {
    fn start(base: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "pane-live-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 30,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_pane"));
        command.args(["session", "--root"]);
        command.arg(&root);
        command.args(["--model", "fixture-model", "--glasshouse"]);
        command.arg(root.join("no-glasshouse"));
        command.env("ANTHROPIC_BASE_URL", base);
        command.env_remove("ANTHROPIC_API_KEY");
        command.env_remove("ANTHROPIC_AUTH_TOKEN");
        command.env("TERM", "xterm-256color");
        #[cfg(unix)]
        let terminal_flags = pair
            .master
            .get_termios()
            .unwrap()
            .local_flags
            .bits()
            .to_ne_bytes()
            .to_vec();
        let child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let input = pair.master.take_writer().unwrap();
        let (sender, output) = mpsc::channel();
        thread::spawn(move || {
            let mut buf = [0; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if sender.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Self {
            master: pair.master,
            child,
            input,
            output,
            screen: vt100::Parser::new(30, 80, 1000),
            bytes: Vec::new(),
            root,
            #[cfg(unix)]
            terminal_flags,
        }
    }
    fn send(&mut self, bytes: &[u8]) {
        self.input.write_all(bytes).unwrap();
        self.input.flush().unwrap();
    }
    fn wait(&mut self, description: &str, predicate: impl Fn(&vt100::Screen) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if predicate(self.screen.screen()) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{description}:\n{}",
                self.screen.screen().contents()
            );
            if let Ok(bytes) = self.output.recv_timeout(Duration::from_millis(25)) {
                self.answer_cursor_query(&bytes);
                self.screen.process(&bytes);
                self.bytes.extend(bytes);
            }
        }
    }
    /// Reply to `ESC[6n` the way a real terminal does, with the cursor's
    /// position. **Row 1, column 1 is a true answer here**, not a placeholder:
    /// the emulator this fixture keeps is the only screen there is, and the
    /// cursor starts at its origin.
    fn answer_cursor_query(&mut self, bytes: &[u8]) {
        if bytes.windows(4).any(|w| w == b"\x1b[6n") {
            let _ = self.input.write_all(b"\x1b[1;1R");
            let _ = self.input.flush();
        }
    }

    fn contains(&mut self, needle: &str) {
        self.wait(needle, |screen| screen.contents().contains(needle));
    }
    /// Apply whatever the session has emitted so far. An assertion about the
    /// *absence* of text needs this: `wait` stops pumping the moment its
    /// predicate holds, so a screen that was never brought up to date can
    /// satisfy a `!contains` for the wrong reason.
    fn settle(&mut self, millis: u64) {
        let deadline = Instant::now() + Duration::from_millis(millis);
        while Instant::now() < deadline {
            if let Ok(bytes) = self.output.recv_timeout(Duration::from_millis(25)) {
                self.answer_cursor_query(&bytes);
                self.screen.process(&bytes);
                self.bytes.extend(bytes);
            }
        }
    }
    /// Send an SGR mouse report the way the defect arrives in a real PTY: the
    /// Escape in one write, the printable remainder in the next.
    ///
    /// **This is a smoke test, not a control.** Nothing here can make the
    /// reader split at a chosen boundary: on an idle machine both writes are
    /// usually drained in one read and the split never happens, and on a
    /// loaded one the pause is whatever the scheduler gives. That is exactly
    /// how a timing-dependent reassembler shipped green from here and failed
    /// on a CI runner. The boundary-by-boundary coverage is the unit tests in
    /// `session::ui::terminal_input`, which feed the reassembler directly.
    fn send_split_mouse_report(&mut self, tail: &[u8]) {
        self.send_report_split_by(tail, Duration::from_millis(5));
    }
    /// The same, with the gap named. A *large* gap is the control the 5 ms one
    /// is not: pane polls the terminal every 40--100 ms, so a third of a
    /// second between the two writes cannot land in one read.
    fn send_report_split_by(&mut self, tail: &[u8], gap: Duration) {
        self.send(b"\x1b");
        thread::sleep(gap);
        self.send(tail);
    }
    fn resize(&mut self, width: u16) {
        self.screen.screen_mut().set_size(30, width);
        self.master
            .resize(PtySize {
                rows: 30,
                cols: width,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
    }
    fn exited(&mut self) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                while let Ok(bytes) = self.output.recv_timeout(Duration::from_millis(30)) {
                    self.screen.process(&bytes);
                    self.bytes.extend(bytes);
                }
                #[cfg(unix)]
                assert_eq!(
                    self.master
                        .get_termios()
                        .unwrap()
                        .local_flags
                        .bits()
                        .to_ne_bytes()
                        .to_vec(),
                    self.terminal_flags,
                    "raw terminal mode was not restored"
                );
                return status.exit_code();
            }
            assert!(
                Instant::now() < deadline,
                "session did not exit: {}",
                self.screen.screen().contents()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for App {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn provider() -> (String, mpsc::Receiver<serde_json::Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (sender, requests) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut len = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                len = value.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; len];
        reader.read_exact(&mut body).unwrap();
        let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let streaming = request["stream"] == true;
        sender.send(request).unwrap();
        thread::sleep(Duration::from_millis(700));
        let body=serde_json::json!({"role":"assistant","content":[{"type":"text","text":"```pane\nreturn \"LIVE RESULT INTACT\";\n```"}],"usage":{"input_tokens":123,"output_tokens":12}}).to_string();
        if streaming {
            let body: serde_json::Value = serde_json::from_str(&body).unwrap();
            let response = body["content"][0]["text"].as_str().unwrap();
            let events = [
                serde_json::json!({"type":"message_start","message":{"role":"assistant","usage":{"input_tokens":123}}}),
                serde_json::json!({"type":"content_block_delta","delta":{"type":"text_delta","text":response}}),
                serde_json::json!({"type":"message_delta","usage":{"output_tokens":12}}),
                serde_json::json!({"type":"message_stop"}),
            ];
            let body = events
                .iter()
                .map(|e| format!("data: {e}\n\n"))
                .collect::<String>();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        } else {
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
    });
    (base, requests)
}

#[test]
fn live_composition_completion_model_selection_busy_input_resize_and_exit() {
    let (base, requests) = provider();
    let mut app = App::start(&base);
    app.contains("fixture-model");
    app.contains("sandbox 0p/0c");
    assert!(app.screen.screen().alternate_screen());
    app.send(b"/theme amber\r");
    app.contains("Theme: amber");
    app.send(b"/effort medium\r");
    app.contains("Effort: medium");
    app.send(b"/mo");
    app.contains("select the active model");
    app.send(b"\tfixture-next\r");
    app.contains("model changed to fixture-next");
    app.send(b"\x1b[200~first line\nsecond line\x1b[201~");
    app.contains("second line");
    assert!(app.screen.screen().contents().contains("first line"));
    app.send(b"\x15answer this\r");
    let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(request["model"], "fixture-next");
    assert_eq!(request["thinking"]["budget_tokens"], 16384);
    assert!(request["max_tokens"].as_u64().unwrap() > 16384);
    app.contains("thinking");
    app.send(b"next draft");
    app.contains("next draft");
    app.contains("complete");
    app.contains("LIVE RESULT INTACT");
    assert!(app.screen.screen().contents().contains("next draft"));
    for width in [60, 80, 120, 200] {
        app.resize(width);
        app.contains("LIVE RESULT INTACT");
        app.wait("composer survives resize", |screen| {
            screen.contents().contains("next draft") && screen.contents().contains("sandbox 0p/0c")
        });
        if width >= 120 {
            app.contains("telemetry");
        }
    }
    app.send(b"\x02");
    app.wait("sidebar hidden", |screen| {
        !screen.contents().contains("telemetry")
    });
    app.send(b"\x02");
    app.contains("telemetry");
    app.send(b"\x15/exit\r");
    assert_eq!(app.exited(), 0);
    assert!(!app.screen.screen().alternate_screen());
    assert!(app.bytes.windows(8).any(|bytes| bytes == b"\x1b[?2004h"));
}

#[test]
fn a_request_error_is_visible_and_the_editor_remains_usable() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let mut app = App::start(&base);
    app.contains("fixture-model");
    app.send(b"fail this\r");
    app.contains("ERROR:");
    app.contains("request failed");
    app.send(b"/theme amber\r");
    app.contains("Theme: amber");
    app.send(b"/effort medium\r");
    app.contains("Effort: medium");
    app.send(b"/mo");
    app.contains("select the active model");
    app.send(b"\x15/exit\r");
    assert_eq!(app.exited(), 0);
    assert!(!app.screen.screen().alternate_screen());
}

#[test]
fn double_ctrl_c_restores_the_terminal_before_exit() {
    let mut app = App::start("http://127.0.0.1:1");
    app.contains("fixture-model");
    app.send(b"\x03");
    thread::sleep(Duration::from_millis(100));
    app.send(b"\x03");
    assert_eq!(app.exited(), 130);
    assert!(!app.screen.screen().alternate_screen());
}

#[test]
fn shift_tab_enters_a_real_nonexecuting_plan_mode() {
    let (base, requests) = provider();
    let mut app = App::start(&base);
    app.contains("fixture-model");
    app.send(b"\x1b[Z");
    app.contains("Mode: plan");
    app.send(b"plan this\r");
    let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        request["system"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Planning mode")
    );
    app.contains("Planning mode");
    app.contains("code was not executed");
    assert!(!app.screen.screen().contents().contains("PANE / RETURN"));
    app.send(b"\x1b[Z");
    app.contains("Mode: execute");
    app.send(b"/context\r");
    app.contains("Next request:");
    app.send(b"\x1b");
    app.wait("context panel closes", |screen| {
        !screen.contents().contains("Next request:")
    });
    app.send(b"/statusline compact\r");
    app.contains("fixture-model");
    app.send(b"/exit\r");
    assert_eq!(app.exited(), 0);
}

#[cfg(unix)]
#[test]
fn model_picker_sorts_accounts_and_selects_a_real_request_model() {
    use std::os::unix::fs::PermissionsExt;
    let (base, requests) = provider();
    let mut app = App::start(&base);
    app.contains("fixture-model");
    let executable = app.root.join("no-glasshouse");
    std::fs::write(&executable, "#!/bin/sh\nprintf '%s\\n' '{\"version\":1,\"accounts\":[{\"account\":\"z-account\",\"provider\":\"fixture\",\"models\":[\"z-model\"],\"scope\":\"provider-declared\"},{\"account\":\"a-account\",\"provider\":\"fixture\",\"models\":[\"b-model\",\"a-model\"],\"scope\":\"provider-declared\"}]}'\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    app.send(b"/model\r");
    app.contains("Models by provider");
    app.contains("a-model");
    let content = app.screen.screen().contents();
    assert!(content.find("a-account").unwrap() < content.find("z-account").unwrap());
    assert!(content.find("a-model").unwrap() < content.find("b-model").unwrap());
    app.send(b"\r");
    app.contains("model changed to a-model");
    app.send(b"answer this\r");
    let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(request["model"], "a-model");
    app.contains("LIVE RESULT INTACT");
    app.send(b"/exit\r");
    assert_eq!(app.exited(), 0);
}

#[cfg(unix)]
#[test]
fn model_picker_searches_a_large_catalogue_and_applies_the_filtered_selection() {
    use std::os::unix::fs::PermissionsExt;
    let (base, requests) = provider();
    let mut app = App::start(&base);
    app.contains("fixture-model");
    let models: Vec<_> = (0..304)
        .rev()
        .map(|i| format!("vendor/model-{i:03}"))
        .collect();
    let catalogue = serde_json::json!({"version": 1, "accounts": [
        {"account":"personal", "provider":"openrouter", "scope":"provider-declared", "models":models},
        {"account":"work", "provider":"openrouter", "scope":"provider-declared", "models":["vendor/model-303"]},
        {"account":"google-sub", "provider":"google", "scope":"subscription", "models":["gemini/exact"]},
        {"account":"claude-sub", "provider":"anthropic", "scope":"subscription", "models":["claude/exact"], "selectable":false, "unavailable_reason":"Pinned to another entitlement"},
        {"account":"openai-sub", "provider":"openai", "scope":"subscription", "models":["gpt/exact"]}
    ]});
    std::fs::write(app.root.join("catalogue.json"), catalogue.to_string()).unwrap();
    let executable = app.root.join("no-glasshouse");
    std::fs::write(
        &executable,
        format!(
            "#!/bin/sh\ncat '{}'\n",
            app.root.join("catalogue.json").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    app.send(b"/model\r");
    app.contains("308/308");
    app.contains("gemini/exact");
    app.send(b"\x1b[D");
    app.contains("claude/exact");
    app.contains("locked");
    app.contains("Pinned to another entitlement");
    app.send(b"\r");
    assert!(requests.try_recv().is_err());
    app.contains("▶");
    app.send(b"\x1b[C");
    app.contains("gemini/exact");
    assert!(!app.screen.screen().contents().contains("claude/exact"));
    app.send(b"OPENROUTER work 303");
    app.contains("1/308");
    app.contains("openrouter · work");
    app.contains("vendor/model-303");
    assert!(!app.screen.screen().contents().contains("personal"));
    app.send(b"x");
    app.contains("No models match");
    app.send(b"\r");
    assert!(requests.try_recv().is_err());
    app.send(b"\x7f");
    app.contains("1/308");
    app.send(b"\x15");
    app.contains("308/308");
    app.send(b"\x1b[200~personal 302\x1b[201~");
    app.contains("vendor/model-302");
    app.contains("1/308");
    app.resize(40);
    app.contains("vendor/model-302");
    app.send(b"\r");
    app.contains("model changed to vendor/model-302");
    app.send(b"answer this\r");
    let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(request["model"], "vendor/model-302");
    app.contains("LIVE RESULT INTACT");
    app.send(b"/exit\r");
    assert_eq!(app.exited(), 0);
}

#[test]
fn telemetry_and_motion_are_local_controls_with_real_response_usage() {
    let (base, requests) = provider();
    let mut app = App::start(&base);
    app.contains("fixture-model");
    app.send(b"/motion off\r");
    app.contains("Motion reduced");
    app.send(b"/telemetry\r");
    app.contains("LIVE INSTRUMENTS");
    assert!(requests.try_recv().is_err());
    app.send(b"answer this\r");
    let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(request["model"], "fixture-model");
    app.contains("REQUEST 01");
    app.contains("input 123");
    app.contains("output 12");
    app.contains("cost unreported");
    app.contains("1 deliveries");
    app.send(b"\x14");
    app.contains("LIVE RESULT INTACT");
    app.send(b"\x14");
    app.contains("LIVE INSTRUMENTS");
    app.send(b"next draft");
    for width in [60, 80, 120, 200] {
        app.resize(width);
        app.wait("redraw after resize", |screen| {
            // The current-context reading is the statusline's highest-priority
            // right-edge signal and must survive even the narrow layout.
            (25..30).any(|row| {
                screen
                    .contents_between(row, 0, row, width)
                    .contains("ctx 123")
            })
        });
        app.contains("REQUEST 01");
        app.contains("next draft");
    }
    app.send(b"\x1b");
    app.contains("LIVE RESULT INTACT");
    app.contains("next draft");
    app.send(b"\x15/exit\r");
    assert_eq!(app.exited(), 0);
}

#[test]
fn theme_picker_applies_local_palettes_without_a_request() {
    let mut app = App::start("http://127.0.0.1:1");
    app.contains("fixture-model");
    app.send(b"/theme\r");
    app.contains("Themes");
    app.contains("violet");
    app.send(b"\x1b[B\x1b[B\x1b[B\x1b[B\r");
    app.contains("Theme: violet");
    app.send(b"/theme cobalt\r");
    app.contains("Theme: cobalt");
    app.send(b"/theme mint\r");
    app.contains("Theme: mint");
    app.send(b"/theme rose\r");
    app.contains("Theme: rose");
    app.send(b"/exit\r");
    assert_eq!(app.exited(), 0);
}

/// Only a real terminal can show that Ctrl-F reaches `screen_regions` at all:
/// the structural test proves the layout, and this proves the key is bound and
/// that the composer it leaves behind still accepts and keeps a draft.
#[test]
fn ctrl_f_takes_the_screen_and_gives_it_back_with_the_draft_intact() {
    let mut app = App::start("http://127.0.0.1:1");
    app.contains("PANE /");
    app.contains("sandbox 0p/0c");
    app.send(b"a draft mid-thought");
    app.contains("a draft mid-thought");
    app.send(b"\x06");
    app.wait("header and status gone after Ctrl-F", |screen| {
        let screen = screen.contents();
        !screen.contains("PANE /") && !screen.contains("sandbox 0p/0c")
    });
    // The composer is not part of the hide-set, and neither is what is in it.
    app.contains("a draft mid-thought");
    app.send(b" still typing");
    app.contains("a draft mid-thought still typing");
    app.send(b"\x06");
    app.contains("PANE /");
    app.contains("sandbox 0p/0c");
    app.contains("a draft mid-thought still typing");
    app.send(b"\x15/exit\r");
    assert_eq!(app.exited(), 0);
}

fn emitted(stream: &[u8], needle: &[u8]) -> bool {
    stream.windows(needle.len()).any(|bytes| bytes == needle)
}

/// The modes pane asks the terminal for are the modes its own event loop
/// reads. `?1002`/`?1003` would report every pointer movement over the window
/// into a handler that only matches the wheel, and each of those reports is
/// another chance for a read boundary to split one into the composer.
#[test]
fn mouse_reporting_asks_only_for_the_modes_the_ui_consumes() {
    let mut app = App::start("http://127.0.0.1:1");
    app.contains("PANE /");
    let startup = app.bytes.len();
    assert!(
        emitted(&app.bytes, b"\x1b[?1000h"),
        "press/release reporting must be requested"
    );
    assert!(
        emitted(&app.bytes, b"\x1b[?1006h"),
        "SGR encoding must be requested"
    );
    app.send(b"/exit\r");
    assert_eq!(app.exited(), 0);
    let shutdown = app.bytes[startup..].to_vec();
    assert!(
        emitted(&shutdown, b"\x1b[?1000l") && emitted(&shutdown, b"\x1b[?1006l"),
        "both requested modes must be reset on exit"
    );
    for unused in [b"?1002".as_slice(), b"?1003".as_slice()] {
        assert!(
            !emitted(&app.bytes, unused),
            "a motion mode nothing handles was negotiated: {}",
            String::from_utf8_lossy(unused)
        );
    }
}

/// A click is the case the wheel-only repair missed. Before it, the tail of a
/// split `[<0;10;5M` failed the wheel test, was queued behind the Escape and
/// typed: the composer held `[<0;10;5M[<0;10;5m` and the model was sent it.
///
/// Smoke only — `send_split_mouse_report` cannot guarantee the split, so a
/// green run here does not prove the reassembly. That proof is
/// `a_report_split_at_any_boundary_is_never_typed` in the unit tests.
#[test]
fn a_fragmented_click_report_does_not_become_prompt_text() {
    let (base, requests) = provider();
    let mut app = App::start(&base);
    app.contains("fixture-model");
    app.send_split_mouse_report(b"[<0;10;5M");
    app.send_split_mouse_report(b"[<0;10;5m");
    app.settle(200);
    let screen = app.screen.screen().contents();
    assert!(
        !screen.contains("[<0"),
        "report reached the screen:\n{screen}"
    );
    assert!(
        !screen.contains("10;5"),
        "report reached the screen:\n{screen}"
    );
    app.send(b"CLICK_INPUT_OK\r");
    let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        request["messages"][0]["content"][0]["text"],
        "CLICK_INPUT_OK"
    );
    app.contains("LIVE RESULT INTACT");
    app.send(b"/exit\r");
    assert_eq!(app.exited(), 0);
}

/// **The one PTY test here that controls the split.** Five milliseconds is a
/// hope — both writes usually drain in a single read, and then nothing is
/// split at all — but 300 ms cannot be: pane polls every 40--100 ms, so the
/// Escape is certainly read alone and the tail certainly arrives in a later
/// read, long after any timer a reassembler could have kept. This is the
/// condition CI created by accident on a loaded runner, and the condition the
/// timing-based predecessor lost under: it typed `[<65;101;28M` into the
/// composer and sent it to the model. Holding a run by the grammar rather than
/// by a clock is what makes the size of the gap irrelevant.
#[test]
fn a_report_whose_halves_are_a_third_of_a_second_apart_is_still_not_typed() {
    let (base, requests) = provider();
    let mut app = App::start(&base);
    app.contains("fixture-model");
    let gap = Duration::from_millis(300);
    app.send_report_split_by(b"[<65;101;28M", gap);
    app.send_report_split_by(b"[<0;10;5M", gap);
    app.send_report_split_by(b"[<0;10;5m", gap);
    app.settle(200);
    let screen = app.screen.screen().contents();
    for leak in ["[<65", "[<0;", "101;28", "10;5"] {
        assert!(
            !screen.contains(leak),
            "report reached the screen:\n{screen}"
        );
    }
    app.send(b"SLOW_SPLIT_OK\r");
    let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        request["messages"][0]["content"][0]["text"],
        "SLOW_SPLIT_OK"
    );
    app.contains("LIVE RESULT INTACT");
    app.send(b"/exit\r");
    assert_eq!(app.exited(), 0);
}

/// The wheel half of the same repair, proved by its effect rather than by the
/// absence of text: a transcript taller than the viewport scrolls back to its
/// first line under fragmented wheel-up reports. What only a real terminal can
/// show is that the reassembled event reaches the scroll handler at all; that
/// it survives *every* split boundary is the unit tests' job, not this one's.
#[test]
fn a_fragmented_wheel_report_still_scrolls_the_transcript() {
    let mut app = App::start("http://127.0.0.1:1");
    app.contains("PANE /");
    let mut paste = b"\x1b[200~TOP_OF_TRANSCRIPT".to_vec();
    for line in 0..60 {
        paste.extend(format!("\nfiller {line:02}").as_bytes());
    }
    paste.extend(b"\x1b[201~");
    app.send(&paste);
    app.contains("filler 59");
    app.send(b"\r");
    app.contains("ERROR:");
    app.wait("the first line leaves the viewport", |screen| {
        !screen.contents().contains("TOP_OF_TRANSCRIPT")
    });
    for _ in 0..30 {
        app.send_split_mouse_report(b"[<64;10;5M");
        app.settle(20);
        if app.screen.screen().contents().contains("TOP_OF_TRANSCRIPT") {
            break;
        }
    }
    app.contains("TOP_OF_TRANSCRIPT");
    app.send(b"/exit\r");
    assert_eq!(app.exited(), 0);
}

/// The report must not reach the composer, and the text typed after it must
/// arrive alone. Smoke only, for the reason in `send_split_mouse_report`: this
/// test was green on every local run of the timing-based reassembler and red
/// on a loaded CI runner, where the composer held
/// `[<65;101;28MWHEEL_INPUT_OK`.
#[test]
fn fragmented_mouse_reports_do_not_become_prompt_text() {
    let (base, requests) = provider();
    let mut app = App::start(&base);
    app.contains("fixture-model");
    app.send_split_mouse_report(b"[<65;101;28M");
    app.send(b"WHEEL_INPUT_OK\r");
    let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        request["messages"][0]["content"][0]["text"],
        "WHEEL_INPUT_OK"
    );
    app.contains("LIVE RESULT INTACT");
    app.send(b"/exit\r");
    assert_eq!(app.exited(), 0);
}

#[test]
fn handlers_can_be_inspected_and_cancelled_during_an_active_task() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (waiting, requests) = mpsc::channel();
    let (release, allowed) = mpsc::channel();
    thread::spawn(move || {
        for turn in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut bytes = vec![0; length];
            reader.read_exact(&mut bytes).unwrap();
            if turn == 1 {
                waiting.send(()).unwrap();
                allowed.recv_timeout(Duration::from_secs(15)).unwrap();
            }
            let text = if turn == 0 {
                "```pane\nconst noise = on({}, 'batch.ack(batch.rest().map(e => e.id));');\n```"
            } else {
                "```pane\nreturn 'HANDLER CONTROL DONE';\n```"
            };
            let events = [
                serde_json::json!({"type":"message_start","message":{"role":"assistant","usage":{"input_tokens":20}}}),
                serde_json::json!({"type":"content_block_delta","delta":{"type":"text_delta","text":text}}),
                serde_json::json!({"type":"message_delta","usage":{"output_tokens":12}}),
                serde_json::json!({"type":"message_stop"}),
            ];
            let body = events
                .iter()
                .map(|e| format!("data: {e}\n\n"))
                .collect::<String>();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
    });
    let mut app = App::start(&base);
    app.contains("fixture-model");
    app.send(b"register noise handler\r");
    requests.recv_timeout(Duration::from_secs(10)).unwrap();
    app.send(b"/handlers\r");
    app.contains("Standing handlers");
    app.contains("noise");
    app.contains("active");
    app.send(b"\x1b");
    app.wait("handler panel closed", |screen| {
        !screen.contents().contains("Standing handlers")
    });
    app.send(b"/handlers off noise\r");
    app.contains("cancellation queued");
    app.send(b"/handlers\r");
    app.contains("Standing handlers");
    app.contains("noise");
    app.resize(40);
    app.contains("Standing handlers");
    release.send(()).unwrap();
    // Keep the panel open across task completion, including on a narrow
    // terminal. Reopening it would conceal a stale snapshot regression.
    app.contains("No handlers in this task");
    app.send(b"\x1b");
    app.wait("completed handler panel closed", |screen| {
        !screen.contents().contains("Standing handlers")
    });
    app.resize(80);
    app.contains("HANDLER CONTROL DONE");
    app.send(b"/handles\r");
    app.contains("Last handle preview");
    app.contains("stale");
    app.send(b"\x1b");
    app.wait("handle panel closed", |screen| {
        !screen.contents().contains("Last handle preview")
    });
    app.send(b"/handlers\r");
    app.contains("No handlers in this task");
    app.send(b"\x1b");
    app.wait("empty handler panel closed", |screen| {
        !screen.contents().contains("Standing handlers")
    });
    app.send(b"/exit\r");
    assert_eq!(app.exited(), 0);
}
