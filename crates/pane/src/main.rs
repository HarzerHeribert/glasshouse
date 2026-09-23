mod cli_workflows;

const HELP: &str = "pane — the Glasshouse native harness

Usage:
  pane
  pane -p <task> [session options]
  pane exec [task] [session options]
  pane --resume [id] | --continue | --sessions
  pane doctor [--root <path>] [--json]
  pane update [--check]
  pane config [global|local] [key] [value] [--root <path>]
  pane session --root <path> [options]
  pane ruler run [options]
  pane --help
  pane --version

Running `pane` with no arguments starts a session in the current project.
`exec` without a task reads all of stdin as one task. Piped input to ordinary
sessions remains one turn per line. Session options default --root to .";

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
        return dispatch_session(&cli_workflows::session_options(&args[1..]));
    }
    if args.first().map(String::as_str) == Some("update") {
        std::process::exit(pane::update::command(&args[1..]));
    }
    if args.first().map(String::as_str) == Some("doctor") {
        std::process::exit(cli_workflows::doctor(&args[1..]));
    }
    if args.first().map(String::as_str) == Some("config") {
        match pane::settings_commands::cli(&args[1..]) {
            Ok(message) => println!("{message}"),
            Err(message) => {
                eprintln!("pane config: {message}");
                std::process::exit(2);
            }
        }
        return Ok(());
    }
    match cli_workflows::prepare(&args) {
        Ok(Some(args)) => return dispatch_session(&args),
        Ok(None) => {}
        Err(message) => {
            eprintln!("pane: {message}");
            std::process::exit(2);
        }
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
