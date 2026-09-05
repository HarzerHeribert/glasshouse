use std::io::{stdin, stdout};

fn main() -> std::io::Result<()> {
    let mut input = stdin().lock();
    let mut output = stdout().lock();
    pane::echo_line(&mut input, &mut output)?;
    Ok(())
}
