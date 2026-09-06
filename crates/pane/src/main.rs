use std::io::{stdin, stdout};

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `pane --version` prints the crate version and nothing else: a release
    // archive can be told from a build, and the primary asked for it (07:16).
    if matches!(args.first().map(String::as_str), Some("--version" | "-V")) {
        println!("pane {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("ruler") {
        if let Err(message) = pane::ruler::cli::dispatch(&args[1..]) {
            eprintln!("{message}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("session") {
        if let Err(message) = pane::session::dispatch(&args[1..]) {
            eprintln!("{message}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let mut input = stdin().lock();
    let mut output = stdout().lock();
    pane::echo_line(&mut input, &mut output)?;
    Ok(())
}
