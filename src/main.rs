mod config;
mod repl;
mod api;

use anyhow::Result;
use std::path::PathBuf;

fn main() -> Result<()> {
    println!("🐚 apish - Interactive API Shell");
    println!("Type 'help' for available commands, 'exit' to quit\n");

    // Initialize config directory
    let config_dir = dirs::config_dir()
        .map(|p| p.join("apish"))
        .unwrap_or_else(|| PathBuf::from(".apish"));

    std::fs::create_dir_all(&config_dir)?;

    // Start REPL
    repl::start(config_dir)?;

    Ok(())
}
