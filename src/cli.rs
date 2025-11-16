//! Command-line interface and REPL

use anyhow::Result;
use rustyline::DefaultEditor;

pub async fn run_repl() -> Result<()> {
    let mut rl = DefaultEditor::new()?;
    
    loop {
        let readline = rl.readline("xtouch> ");
        match readline {
            Ok(line) => {
                if line.trim() == "exit" || line.trim() == "quit" {
                    break;
                }
                // Process command
                println!("Command: {}", line);
            }
            Err(_) => break,
        }
    }
    
    Ok(())
}
