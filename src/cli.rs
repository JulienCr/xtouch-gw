//! Command-line interface and REPL

use anyhow::Result;
use rustyline::DefaultEditor;

pub async fn run_repl() -> Result<()> {
    let mut rl = DefaultEditor::new()?;
    
    loop {
        let readline = rl.readline("xtouch> ");
        match readline {
            Ok(line) => {
                let trimmed = line.trim();

                if trimmed == "exit" || trimmed == "quit" {
                    break;
                }

                if trimmed == "$" {
                    // Clear the terminal (works on ANSI terminals incl. Windows 10+)
                    // Move cursor to home after clearing.
                    print!("\x1B[2J\x1B[H");
                    // Ensure the clear is flushed immediately.
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                    continue;
                }

                // Process command
                println!("Command: {}", line);
            }
            Err(_) => break,
        }
    }
    
    Ok(())
}
