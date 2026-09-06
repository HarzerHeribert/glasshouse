//! Real PTY + terminal-emulator coverage; no external model or credentials.
//!
//! **Unix only, and that is a stated limitation rather than a tidy-up.** Every
//! test here drives the shipped binary through a real PTY and reads the screen
//! back through a terminal emulator. On Windows the whole file produced a blank
//! screen and timed out: `session` starts its interactive interface only when
//! stdin *and* stdout are terminals, and under ConPTY as `portable-pty` attaches
//! it that did not hold, so pane fell through to line mode and drew nothing.
//!
//! **What this costs: pane's interactive terminal is unverified on Windows and
//! may not start there at all.** Nothing else covers it — `tui.rs` renders into
//! a `TestBackend` and never asks whether a real terminal was detected. Closing
//! that needs a Windows host to debug on, which this project does not have for
//! pane (`rusty_v8` wants MSVC, so even the local cross-check cannot build these
//! targets). Gating it makes the gap visible instead of red.
#![cfg(unix)]
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
                self.screen.process(&bytes);
                self.bytes.extend(bytes);
            }
        }
    }
    fn contains(&mut self, needle: &str) {
        self.wait(needle, |screen| screen.contents().contains(needle));
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
        request["system"]
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
    app.contains("Models by entitlement");
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
            // The mode moves to the new right edge only after a real redraw.
            (25..30).any(|row| {
                screen
                    .contents_between(row, width - 14, row, width)
                    .contains("effort auto")
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
