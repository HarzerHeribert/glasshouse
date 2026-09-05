use std::io::{stdin, stdout};

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("ruler") {
        if let Err(message) = pane::ruler::cli::dispatch(&args[1..]) {
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
