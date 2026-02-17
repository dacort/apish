use crate::api::ApiClient;
use crate::config::{ApiConfig, History};
use anyhow::{Result, anyhow};
use colored::Colorize;
use rustyline::error::ReadlineError;
use rustyline::{DefaultEditor, Config, CompletionType};
use std::collections::HashMap;
use std::path::PathBuf;

pub struct ReplContext {
    config_dir: PathBuf,
    apis: HashMap<String, ApiConfig>,
    current_api: Option<String>,
    current_path: String,
    history: History,
}

impl ReplContext {
    pub fn new(config_dir: PathBuf) -> Result<Self> {
        // Load all API configs
        let mut apis = HashMap::new();
        let configs = ApiConfig::load_all_from_dir(&config_dir)?;
        for config in configs {
            apis.insert(config.name.clone(), config);
        }

        // Load history
        let history_path = config_dir.join("history.json");
        let history = History::load(&history_path).unwrap_or_else(|_| History { entries: Vec::new() });

        Ok(Self {
            config_dir,
            apis,
            current_api: None,
            current_path: "/".to_string(),
            history,
        })
    }

    fn save_history(&self) -> Result<()> {
        let history_path = self.config_dir.join("history.json");
        self.history.save(&history_path)
    }

    fn get_prompt(&self) -> String {
        if let Some(api) = &self.current_api {
            format!("{}:{} > ", api.cyan().bold(), self.current_path.green())
        } else {
            format!("{} > ", "apish".cyan().bold())
        }
    }

    fn execute_command(&mut self, command: &str) -> Result<bool> {
        let parts: Vec<&str> = command.trim().split_whitespace().collect();
        if parts.is_empty() {
            return Ok(true);
        }

        match parts[0] {
            "help" | "?" => self.show_help(),
            "exit" | "quit" => return Ok(false),
            "apis" => self.list_apis(),
            "use" => self.use_api(&parts[1..])?,
            "cd" => self.change_path(&parts[1..])?,
            "pwd" => println!("{}", self.current_path),
            "ls" => self.list_endpoints()?,
            "get" | "GET" => self.http_request("GET", &parts[1..])?,
            "post" | "POST" => self.http_request("POST", &parts[1..])?,
            "put" | "PUT" => self.http_request("PUT", &parts[1..])?,
            "delete" | "DELETE" => self.http_request("DELETE", &parts[1..])?,
            "patch" | "PATCH" => self.http_request("PATCH", &parts[1..])?,
            "history" => self.show_history(),
            "config" => self.show_config()?,
            ":w" => self.save_config(&parts[1..])?,
            _ => println!("{} Unknown command: {}. Type 'help' for available commands.", "✗".red(), parts[0]),
        }

        Ok(true)
    }

    fn show_help(&self) {
        println!("{}", "Available commands:".bold());
        println!();
        println!("  {}  - Display this help message", "help".yellow());
        println!("  {}  - List available APIs", "apis".yellow());
        println!("  {}  - Switch to an API (e.g., 'use github')", "use <api>".yellow());
        println!("  {}   - Change current path (e.g., 'cd /users')", "cd <path>".yellow());
        println!("  {}   - Show current path", "pwd".yellow());
        println!("  {}    - List available endpoints for current API", "ls".yellow());
        println!();
        println!("  {}  - Make a GET request", "get <path>".yellow());
        println!("  {} - Make a POST request with JSON body", "post <path> <json>".yellow());
        println!("  {}  - Make a PUT request with JSON body", "put <path> <json>".yellow());
        println!("  {} - Make a PATCH request with JSON body", "patch <path> <json>".yellow());
        println!("  {} - Make a DELETE request", "delete <path>".yellow());
        println!();
        println!("  {} - Show request history", "history".yellow());
        println!("  {} - Show current API configuration", "config".yellow());
        println!("  {} - Save current API config to a YAML file", ":w <file>".yellow());
        println!("  {}  - Exit the shell", "exit".yellow());
        println!();
    }

    fn list_apis(&self) {
        if self.apis.is_empty() {
            println!("{} No APIs configured. Add YAML config files to {}",
                "ℹ".blue(),
                self.config_dir.display());
            println!();
            println!("Example config:");
            println!("---");
            println!("name: myapi");
            println!("base_url: https://api.example.com");
            println!("endpoints:");
            println!("  - /users");
            println!("  - /posts");
            return;
        }

        println!("{}", "Available APIs:".bold());
        for (name, config) in &self.apis {
            let indicator = if self.current_api.as_ref() == Some(name) {
                "→".green()
            } else {
                " ".normal()
            };
            println!("  {} {} - {}", indicator, name.cyan(), config.base_url);
        }
        println!();
    }

    fn use_api(&mut self, args: &[&str]) -> Result<()> {
        if args.is_empty() {
            return Err(anyhow!("Usage: use <api_name>"));
        }

        let api_name = args[0];
        if !self.apis.contains_key(api_name) {
            return Err(anyhow!("API '{}' not found. Use 'apis' to list available APIs.", api_name));
        }

        self.current_api = Some(api_name.to_string());
        self.current_path = "/".to_string();
        println!("{} Switched to API: {}", "✓".green(), api_name.cyan().bold());
        Ok(())
    }

    fn change_path(&mut self, args: &[&str]) -> Result<()> {
        if args.is_empty() {
            self.current_path = "/".to_string();
            return Ok(());
        }

        let path = args[0];
        if path.starts_with('/') {
            self.current_path = path.to_string();
        } else if path == ".." {
            // Go up one level
            let parts: Vec<&str> = self.current_path.trim_end_matches('/').rsplitn(2, '/').collect();
            self.current_path = if parts.len() > 1 {
                parts[1].to_string()
            } else {
                "/".to_string()
            };
            if self.current_path.is_empty() {
                self.current_path = "/".to_string();
            }
        } else {
            // Append to current path
            let mut new_path = self.current_path.trim_end_matches('/').to_string();
            new_path.push('/');
            new_path.push_str(path);
            self.current_path = new_path;
        }

        Ok(())
    }

    fn list_endpoints(&self) -> Result<()> {
        let api_name = self.current_api.as_ref()
            .ok_or_else(|| anyhow!("No API selected. Use 'use <api>' first."))?;

        let config = self.apis.get(api_name).unwrap();

        if config.endpoints.is_empty() {
            println!("{} No endpoints configured for this API", "ℹ".blue());
            println!("You can still make requests to any path using get/post/etc commands");
            return Ok(());
        }

        println!("{}", "Configured endpoints:".bold());
        for endpoint in &config.endpoints {
            println!("  {}", endpoint.green());
        }

        // Also show history for this API
        let history_paths = self.history.get_paths_for_api(api_name);
        if !history_paths.is_empty() {
            println!();
            println!("{}", "Previously accessed paths:".bold());
            for path in history_paths.iter().take(10) {
                println!("  {}", path.yellow());
            }
        }

        println!();
        Ok(())
    }

    fn http_request(&mut self, method: &str, args: &[&str]) -> Result<()> {
        let api_name = self.current_api.as_ref()
            .ok_or_else(|| anyhow!("No API selected. Use 'use <api>' first."))?;

        let config = self.apis.get(api_name).unwrap();

        // Parse path and body
        let path = if args.is_empty() {
            self.current_path.clone()
        } else {
            if args[0].starts_with('/') || args[0].starts_with("http") {
                args[0].to_string()
            } else {
                format!("{}/{}", self.current_path.trim_end_matches('/'), args[0])
            }
        };

        let body = if args.len() > 1 {
            Some(args[1..].join(" "))
        } else {
            None
        };

        // Create client and make request
        let client = ApiClient::new(config.clone())?;

        println!("{} {} {}", "→".blue(), method, path);

        let response = client.request(method, &path, body.as_deref())?;
        let status_code = response.status_code;
        response.print();

        // Auto-add successful paths to endpoints
        if (200..300).contains(&status_code) {
            if let Some(config) = self.apis.get_mut(api_name) {
                if !config.endpoints.contains(&path) {
                    config.endpoints.push(path.clone());
                }
            }
        }

        // Save to history
        self.history.add_entry(
            api_name.clone(),
            method.to_string(),
            path,
            Some(status_code),
        );
        let _ = self.save_history();

        Ok(())
    }

    fn show_history(&self) {
        if self.history.entries.is_empty() {
            println!("{} No request history yet", "ℹ".blue());
            return;
        }

        println!("{}", "Recent requests (last 20):".bold());
        for entry in self.history.entries.iter().rev().take(20) {
            let status = entry.status_code
                .map(|s| format!("{}", s))
                .unwrap_or_else(|| "---".to_string());
            println!("  {} {} {} ({})",
                entry.api.cyan(),
                entry.method.yellow(),
                entry.path.green(),
                status
            );
        }
        println!();
    }

    fn show_config(&self) -> Result<()> {
        let api_name = self.current_api.as_ref()
            .ok_or_else(|| anyhow!("No API selected. Use 'use <api>' first."))?;

        let config = self.apis.get(api_name).unwrap();

        println!("{}", "Current API Configuration:".bold());
        println!("  Name: {}", config.name.cyan());
        println!("  Base URL: {}", config.base_url.green());

        if !config.headers.is_empty() {
            println!("  Headers:");
            for (k, v) in &config.headers {
                println!("    {}: {}", k.yellow(), v);
            }
        }

        if config.auth.is_some() {
            println!("  Auth: {}", "Configured ✓".green());
        }

        println!();
        Ok(())
    }

    fn save_config(&self, args: &[&str]) -> Result<()> {
        let api_name = self.current_api.as_ref()
            .ok_or_else(|| anyhow!("No API selected. Nothing to save."))?;

        let config = self.apis.get(api_name).unwrap();

        if args.is_empty() {
            return Err(anyhow!("Usage: :w <filename>"));
        }

        let mut filename = args[0].to_string();
        if !filename.ends_with(".yaml") && !filename.ends_with(".yml") {
            filename.push_str(".yaml");
        }

        let path = std::path::Path::new(&filename);
        config.save_to_file(path)?;
        println!("{} Saved config to {}", "✓".green(), filename);
        Ok(())
    }
}

pub fn start(config_dir: PathBuf, adhoc_api: Option<ApiConfig>) -> Result<()> {
    let mut ctx = ReplContext::new(config_dir)?;

    // If ad-hoc API provided, insert it and set as current
    if let Some(config) = adhoc_api {
        let name = config.name.clone();
        println!("{} Connected to {} ({})", "✓".green(), name.cyan().bold(), config.base_url);
        ctx.apis.insert(name.clone(), config);
        ctx.current_api = Some(name);
    }

    let config = Config::builder()
        .completion_type(CompletionType::List)
        .build();

    let mut rl = DefaultEditor::with_config(config)?;

    // Load readline history
    let history_file = ctx.config_dir.join("readline_history.txt");
    let _ = rl.load_history(&history_file);

    loop {
        let prompt = ctx.get_prompt();
        match rl.readline(&prompt) {
            Ok(line) => {
                if line.trim().is_empty() {
                    continue;
                }

                let _ = rl.add_history_entry(&line);

                match ctx.execute_command(&line) {
                    Ok(should_continue) => {
                        if !should_continue {
                            break;
                        }
                    }
                    Err(e) => {
                        println!("{} {}", "✗".red(), e);
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                println!("exit");
                break;
            }
            Err(err) => {
                println!("Error: {:?}", err);
                break;
            }
        }
    }

    // Save readline history
    let _ = rl.save_history(&history_file);

    println!("Goodbye! 👋");
    Ok(())
}
