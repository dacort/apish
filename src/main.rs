mod config;
mod repl;
mod api;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use url::Url;

use crate::config::ApiConfig;

#[derive(Parser)]
#[command(name = "apish", about = "Interactive API Shell")]
struct Cli {
    /// Connect directly to an API endpoint (e.g., https://api.example.com)
    #[arg(long)]
    endpoint: Option<String>,

    /// Name for the ad-hoc API (defaults to hostname)
    #[arg(long)]
    name: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    println!("🐚 apish - Interactive API Shell");
    println!("Type 'help' for available commands, 'exit' to quit\n");

    // Initialize config directory
    let config_dir = dirs::config_dir()
        .map(|p| p.join("apish"))
        .unwrap_or_else(|| PathBuf::from(".apish"));

    std::fs::create_dir_all(&config_dir)?;

    // Build ad-hoc API config if --endpoint was provided
    let adhoc_api = if let Some(endpoint) = cli.endpoint {
        let url = Url::parse(&endpoint)
            .map_err(|e| anyhow::anyhow!("Invalid endpoint URL '{}': {}", endpoint, e))?;

        let name = cli.name.unwrap_or_else(|| {
            url.host_str().unwrap_or("adhoc").to_string()
        });

        let base_url = endpoint.trim_end_matches('/').to_string();

        Some(ApiConfig {
            name,
            base_url,
            endpoints: Vec::new(),
            auth: None,
            headers: std::collections::HashMap::new(),
        })
    } else {
        None
    };

    // Start REPL
    repl::start(config_dir, adhoc_api)?;

    Ok(())
}
