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
    /// Every byte the reader thread has taken off the pty, and how many it
    /// had taken when the last `send` was made: a timed-out `wait` reports
    /// the difference, which is what tells a session that never answered
    /// from a fixture that stopped listening.
    received: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    received_at_send: usize,
    sent_at: Instant,
    first_after_send: Option<Duration>,
    reader_ended: std::sync::Arc<std::sync::atomic::AtomicBool>,
    root: PathBuf,
    #[cfg(unix)]
    terminal_flags: Vec<u8>,
}
impl App {
    fn start(base: &str) -> Self {
        Self::start_with(base, false, None)
    }

    fn start_bare(base: &str) -> Self {
        Self::start_with(base, true, None)
    }

    fn start_with_helpers(base: &str, model: &str) -> Self {
        Self::start_with(base, false, Some(model))
    }

    fn start_with(base: &str, bare: bool, helper_model: Option<&str>) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "pane-live-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root).unwrap();
        if let Some(model) = helper_model {
            std::fs::create_dir_all(root.join(".glasshouse")).unwrap();
            std::fs::write(
                root.join(".glasshouse/pane.toml"),
                format!("[helpers]\nmodel = \"{model}\"\npreflight = true\n"),
            )
            .unwrap();
        }
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 30,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_pane"));
        if bare {
            command.cwd(&root);
            // A developer install must not receive this fixture's lifecycle
            // events. The absent command is the normal fail-soft seam.
            command.env("PATH", "");
        } else {
            command.args(["session", "--root"]);
            command.arg(&root);
            command.args(["--model", "fixture-model", "--glasshouse"]);
            command.arg(root.join("no-glasshouse"));
            command.arg("--gateway");
            command.arg(root.join("no-gateway"));
            // Absent: the base URL below is a loopback host, so this session
            // is *hosted*, and a hosted session's catalogue is the gateway
            // binary's. Naming the absent path pins the resolution here
            // rather than at whatever gateway the developer has installed;
            // a picker test writes its script at that path.
            command.env("INFERENCE_GATEWAY_BIN", root.join("no-gateway"));
        }
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
        let received = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reader_ended = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let counted = std::sync::Arc::clone(&received);
        let ended = std::sync::Arc::clone(&reader_ended);
        thread::spawn(move || {
            let mut buf = [0; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        counted.fetch_add(n, std::sync::atomic::Ordering::SeqCst);
                        if sender.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
            ended.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        Self {
            master: pair.master,
            child,
            input,
            output,
            screen: vt100::Parser::new(30, 80, 1000),
            bytes: Vec::new(),
            received,
            received_at_send: 0,
            sent_at: Instant::now(),
            first_after_send: None,
            reader_ended,
            root,
            #[cfg(unix)]
            terminal_flags,
        }
    }
    fn send(&mut self, bytes: &[u8]) {
        self.received_at_send = self.received.load(std::sync::atomic::Ordering::SeqCst);
        self.sent_at = Instant::now();
        self.first_after_send = None;
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
                "{description}:\n{}\n--- {} bytes arrived after the last send (the first of them {:?} \
                 after it); reader thread ended: {}; the last bytes received: {:?} ---",
                self.screen.screen().contents(),
                self.received.load(std::sync::atomic::Ordering::SeqCst) - self.received_at_send,
                self.first_after_send,
                self.reader_ended.load(std::sync::atomic::Ordering::SeqCst),
                String::from_utf8_lossy(&self.bytes[self.bytes.len().saturating_sub(240)..]),
            );
            if let Ok(bytes) = self.output.recv_timeout(Duration::from_millis(25)) {
                if self.first_after_send.is_none() {
                    self.first_after_send = Some(self.sent_at.elapsed());
                }
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
    /// Resize the terminal and return once pane has begun redrawing for the
    /// new size.
    ///
    /// **Returning earlier races the resize itself.** The emulator keeps its
    /// contents across `set_size`, so a caller's next `wait` can pass on the
    /// old frame and its next key is sent while pane still has the `SIGWINCH`
    /// in hand — and crossterm's Unix source returns from a resize without
    /// reading the tty bytes the same poll reported, so that key is stranded
    /// until the one after it (measured: 5 of 12 runs of the telemetry test
    /// under 12 busy loops; the same crossterm defect Glasshouse's
    /// `tui/event.rs` narrows in its own loop, and a pane packet of its own).
    /// Ratatui begins every post-resize redraw with a clear, so the clear is
    /// what "begun redrawing" means here; the caller still waits for the
    /// content it needs.
    fn resize(&mut self, width: u16) {
        let mark = self.bytes.len();
        self.screen.screen_mut().set_size(30, width);
        self.master
            .resize(PtySize {
                rows: 30,
                cols: width,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.bytes[mark..]
            .windows(4)
            .any(|window| window == b"\x1b[2J")
        {
            assert!(
                Instant::now() < deadline,
                "pane did not begin a redraw (a clear) within 10s of a resize to {width} columns:\n{}",
                self.screen.screen().contents()
            );
            if let Ok(bytes) = self.output.recv_timeout(Duration::from_millis(25)) {
                self.answer_cursor_query(&bytes);
                self.screen.process(&bytes);
                self.bytes.extend(bytes);
            }
        }
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
        // The client is allowed to be gone by now: every test kills pane in
        // `App::drop`, and this thread is still inside its 700 ms sleep when
        // that happens. Windows spells the resulting write `ConnectionReset`
        // rather than `BrokenPipe`, and unwrapping it panicked a detached
        // thread mid-run for no defect at all. The request was already
        // delivered above, so a test that needed it is unaffected, and one
        // that does not gets a quiet exit instead of a panic in the log.
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
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        } else {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (base, requests)
}

/// Accept one request and hold its response until the test releases it. The
/// request notification is sent only after the complete headers and body have
/// arrived, so a screen assertion made after it observes the actual interval
/// in which preflight is blocked on the provider.
fn held_provider() -> (String, mpsc::Receiver<serde_json::Value>, mpsc::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (request_sender, requests) = mpsc::channel();
    let (release, held) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut len = 0;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            if line == "\r\n" || line == "\n" {
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
        request_sender
            .send(serde_json::from_slice(&body).unwrap())
            .unwrap();
        if held.recv().is_err() {
            return;
        }
        let body = serde_json::json!({
            "role":"assistant",
            "content":[{"type":"text","text":"```pane\nreturn \"released\";\n```"}],
        })
        .to_string();
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    });
    (base, requests, release)
}

/// Answer the task turn with an explicit Scout call, then hold that Scout's
/// own request so the real PTY can prove the in-flight lane is wired through.
fn held_cell_helper_provider() -> (String, mpsc::Receiver<serde_json::Value>, mpsc::Sender<()>) {
    fn read_request(stream: &mut std::net::TcpStream) -> serde_json::Value {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut len = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" || line == "\n" {
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
        serde_json::from_slice(&body).unwrap()
    }

    fn answer_task(stream: &mut std::net::TcpStream, request: &serde_json::Value) {
        let program =
            "```pane\nconst found = await helper.find(\"find the needle\");\nreturn found;\n```";
        if request["stream"] == true {
            let events = [
                serde_json::json!({"type":"message_start","message":{"role":"assistant","usage":{"input_tokens":12}}}),
                serde_json::json!({"type":"content_block_delta","delta":{"type":"text_delta","text":program}}),
                serde_json::json!({"type":"message_delta","usage":{"output_tokens":8}}),
                serde_json::json!({"type":"message_stop"}),
            ];
            let body = events
                .iter()
                .map(|event| format!("data: {event}\n\n"))
                .collect::<String>();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        } else {
            let body = serde_json::json!({
                "role":"assistant",
                "content":[{"type":"text","text":program}],
            })
            .to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    }

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (request_sender, requests) = mpsc::channel();
    let (release, held) = mpsc::channel();
    thread::spawn(move || {
        let (mut task, _) = listener.accept().unwrap();
        let request = read_request(&mut task);
        answer_task(&mut task, &request);

        let (mut helper, _) = listener.accept().unwrap();
        let request = read_request(&mut helper);
        request_sender.send(request).unwrap();
        let _ = held.recv();
    });
    (base, requests, release)
}

#[test]
fn live_preflight_shows_the_request_scout_and_actual_effort_before_network_returns() {
    let (base, requests, _release) = held_provider();
    let mut app = App::start_with_helpers(&base, "helper-tier");
    app.contains("fixture-model");
    app.send(b"/effort medium\r");
    app.contains("Effort: medium");
    let task = "find where helper cancellation is implemented";
    app.send(format!("{task}\r").as_bytes());

    let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(request["model"], "helper-tier");
    app.wait(
        "submitted request and Scout visible during preflight",
        |screen| {
            let text = screen.contents();
            text.contains(task)
                && text.contains("PREFLIGHT · SCOUT")
                && text.contains("scanning")
                && text.contains("searching")
                && text.contains("effort medium")
        },
    );

    app.send(b"\x03");
    thread::sleep(Duration::from_millis(100));
    app.send(b"\x03");
    assert_eq!(app.exited(), 130);
    assert!(!app.screen.screen().alternate_screen());
}

#[test]
fn live_cell_helper_shows_its_lane_before_its_provider_returns() {
    let (base, requests, _release) = held_cell_helper_provider();
    let mut app = App::start_with_helpers(&base, "helper-tier");
    app.contains("fixture-model");
    app.send(b"find this\r");

    let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(request["model"], "helper-tier");
    app.wait("in-flight cell helper lane", |screen| {
        let text = screen.contents();
        text.contains("find")
            && text.contains("scanning")
            && text.contains("1 lines")
            && text.contains("executing")
    });

    app.send(b"\x03");
    thread::sleep(Duration::from_millis(100));
    app.send(b"\x03");
    assert_eq!(app.exited(), 130);
    assert!(!app.screen.screen().alternate_screen());
}

#[test]
fn bare_pane_opens_the_live_composer_in_its_current_project() {
    let mut app = App::start_bare("http://127.0.0.1:1");
    app.contains("PANE /");
    app.contains("message or / for commands");
    app.send(b"bare entrypoint draft");
    app.contains("bare entrypoint draft");
    app.send(b"\x15/exit\r");
    assert_eq!(app.exited(), 0);
    assert!(app.root.join(".pane/rollout.jsonl").is_file());
    assert!(!app.screen.screen().alternate_screen());
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
    app.contains("set the parent, helper or subagent model");
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
    app.contains("set the parent, helper or subagent model");
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
    let executable = app.root.join("no-gateway");
    std::fs::write(&executable, "#!/bin/sh\nprintf '%s\\n' '{\"version\":1,\"accounts\":[{\"account\":\"z-account\",\"provider\":\"fixture\",\"models\":[\"z-model\"],\"scope\":\"provider-declared\"},{\"account\":\"a-account\",\"provider\":\"fixture\",\"models\":[\"b-model\",\"a-model\"],\"scope\":\"provider-declared\"}]}'\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    app.send(b"/model\r");
    // The title now names every tier, not just that this is a model list.
    app.contains("helper off");
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
    let executable = app.root.join("no-gateway");
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

/// Unix-only, like its one caller's assertions: see
/// `mouse_reporting_asks_only_for_the_modes_the_ui_consumes`. Left ungated it
/// would be dead code on Windows, and dead code is an error under
/// `-D warnings`.
#[cfg(unix)]
fn emitted(stream: &[u8], needle: &[u8]) -> bool {
    stream.windows(needle.len()).any(|bytes| bytes == needle)
}

/// The modes pane asks the terminal for are the modes its own event loop
/// reads. `?1002`/`?1003` would report every pointer movement over the window
/// into a handler that only matches the wheel, and each of those reports is
/// another chance for a read boundary to split one into the composer.
///
/// **The negotiation this reads is a DECSET one, and DECSET negotiation is not
/// how a Windows host asks for mouse input** — crossterm reports the ANSI form
/// unsupported there and sets `ENABLE_MOUSE_INPUT` on the console handle
/// instead, which writes no byte at all, while the bytes that do reach this
/// pty are conhost's rendering of pane's screen rather than pane's own output
/// (it consumes `?1000h`/`?1006h` into its emulator and asks the outer
/// terminal in its own words). Neither the presence nor the absence of a mode
/// on this wire is a fact about pane there, so the wire assertions are unix's.
/// What Windows still proves is the effect, in
/// `a_fragmented_wheel_report_still_scrolls_the_transcript`, which never reads
/// a byte pane wrote.
#[test]
fn mouse_reporting_asks_only_for_the_modes_the_ui_consumes() {
    let mut app = App::start("http://127.0.0.1:1");
    app.contains("PANE /");
    #[cfg(unix)]
    let startup = app.bytes.len();
    #[cfg(unix)]
    {
        assert!(
            emitted(&app.bytes, b"\x1b[?1000h"),
            "press/release reporting must be requested"
        );
        assert!(
            emitted(&app.bytes, b"\x1b[?1006h"),
            "SGR encoding must be requested"
        );
    }
    app.send(b"/exit\r");
    assert_eq!(app.exited(), 0);
    #[cfg(unix)]
    {
        let shutdown = &app.bytes[startup..];
        assert!(
            emitted(shutdown, b"\x1b[?1000l") && emitted(shutdown, b"\x1b[?1006l"),
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
///
/// **The transcript is built from submitted turns, not from one bracketed
/// paste.** A paste is not how every host delivers a multi-line draft:
/// crossterm reads Windows input as console records and has no `Event::Paste`
/// there at all, so the paste that made this transcript tall on unix left it
/// short on Windows — and the test then failed on its own setup, three lines
/// before it reached the wheel it exists to test. A submitted turn is the same
/// height everywhere.
#[test]
fn a_fragmented_wheel_report_still_scrolls_the_transcript() {
    let mut app = App::start("http://127.0.0.1:1");
    app.contains("PANE /");
    app.send(b"TOP_OF_TRANSCRIPT\r");
    app.contains("ERROR:");
    // Named before it is pushed away, so "the first line left the viewport"
    // can never be satisfied by a first line that was never drawn.
    app.contains("TOP_OF_TRANSCRIPT");
    // Each refused turn adds a user block and an error block, so the viewport
    // fills in a handful; the rest of the budget is slack for a host that
    // renders either one shorter.
    for line in 0..12 {
        if !app.screen.screen().contents().contains("TOP_OF_TRANSCRIPT") {
            break;
        }
        let marker = format!("filler {line:02}");
        app.send(format!("{marker}\r").as_bytes());
        app.contains(&marker);
        app.settle(40);
    }
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
