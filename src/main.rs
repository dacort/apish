mod api;
mod config;
mod openapi;
mod repl;

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

        // Try to detect if the endpoint is an OpenAPI spec
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()?;

        let mut openapi_url = None;
        let mut base_url = endpoint.trim_end_matches('/').to_string();

        if let Ok(response) = client.get(&endpoint).send()
            && let Ok(body) = response.text()
            && openapi::is_openapi_spec(&body)
        {
            openapi_url = Some(endpoint.clone());

            // Derive base_url from the spec's servers field, or fall back to the URL's origin
            let origin = {
                let port_suffix = url.port().map(|p| format!(":{}", p)).unwrap_or_default();
                format!(
                    "{}://{}{}",
                    url.scheme(),
                    url.host_str().unwrap_or("localhost"),
                    port_suffix
                )
            };

            if let Ok((Some(server), _)) = openapi::parse_spec(&body) {
                if server.starts_with("http://") || server.starts_with("https://") {
                    base_url = server.trim_end_matches('/').to_string();
                } else {
                    // Relative server URL — resolve against the spec URL origin
                    base_url =
                        format!("{}{}", origin, server.trim_end_matches('/'));
                }
            } else {
                // No servers in spec — use the origin of the spec URL
                base_url = origin;
            }
        }

        let name = cli
            .name
            .unwrap_or_else(|| url.host_str().unwrap_or("adhoc").to_string());

        Some(ApiConfig {
            name,
            base_url,
            endpoints: Vec::new(),
            openapi: openapi_url,
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
