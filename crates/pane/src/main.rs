const HELP: &str = "pane — the Glasshouse native harness

Usage:
  pane
  pane session --root <path> [options]
  pane ruler run [options]
  pane --help
  pane --version

Running `pane` with no arguments starts a session in the current project.";

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `pane --version` prints the crate version and nothing else: a release
    // archive can be told from a build, and the primary asked for it (07:16).
    if matches!(args.first().map(String::as_str), Some("--version" | "-V")) {
        println!("pane {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if matches!(args.first().map(String::as_str), Some("--help" | "-h")) {
        println!("{HELP}");
        return Ok(());
    }
    if args.is_empty() {
        return dispatch_session(&["--root".into(), ".".into()]);
    }
    if args.first().map(String::as_str) == Some("ruler") {
        if let Err(message) = pane::ruler::cli::dispatch(&args[1..]) {
            eprintln!("{message}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("session") {
        return dispatch_session(&args[1..]);
    }

    let kind = if args[0].starts_with('-') {
        "option"
    } else {
        "command"
    };
    eprintln!("pane: unknown {kind}: {}", args[0]);
    eprintln!("Try 'pane --help' for usage.");
    std::process::exit(2);
}

fn dispatch_session(args: &[String]) -> std::io::Result<()> {
    if let Err(message) = pane::session::dispatch(args) {
        eprintln!("{message}");
        std::process::exit(1);
    }
    Ok(())
}
