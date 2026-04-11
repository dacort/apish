use crate::api::ApiClient;
use crate::config::{ApiConfig, History};
use crate::graphql::GraphQLSchema;
use crate::openapi::{self, EndpointInfo};
use anyhow::{Result, anyhow};
use colored::Colorize;
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{CompletionType, Config, Context, Editor, Helper};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const COMMANDS: &[&str] = &[
    "help", "exit", "quit", "apis", "use", "cd", "pwd", "ls", "get", "post", "put", "delete",
    "patch", "GET", "POST", "PUT", "DELETE", "PATCH", "history", "config", ":w",
];

const HTTP_COMMANDS: &[&str] = &[
    "get", "post", "put", "delete", "patch", "GET", "POST", "PUT", "DELETE", "PATCH",
];

#[derive(Clone)]
struct CompletionState {
    endpoint_paths: Vec<String>,
    api_names: Vec<String>,
}

struct ApiHelper {
    state: Arc<Mutex<CompletionState>>,
}

impl Completer for ApiHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let line_up_to_cursor = &line[..pos];
        let parts: Vec<&str> = line_up_to_cursor.split_whitespace().collect();

        let state = self.state.lock().unwrap();

        // Complete command names
        if parts.is_empty() || (parts.len() == 1 && !line_up_to_cursor.ends_with(' ')) {
            let prefix = parts.first().copied().unwrap_or("");
            let matches: Vec<Pair> = COMMANDS
                .iter()
                .filter(|cmd| cmd.starts_with(prefix))
                .map(|cmd| Pair {
                    display: cmd.to_string(),
                    replacement: cmd.to_string(),
                })
                .collect();
            return Ok((0, matches));
        }

        let cmd = parts[0];

        // Complete API names after "use"
        if cmd == "use" {
            let prefix = if parts.len() > 1 && !line_up_to_cursor.ends_with(' ') {
                parts[1]
            } else {
                ""
            };
            let start = line_up_to_cursor.rfind(prefix).unwrap_or(pos);
            let matches: Vec<Pair> = state
                .api_names
                .iter()
                .filter(|name| name.starts_with(prefix))
                .map(|name| Pair {
                    display: name.clone(),
                    replacement: name.clone(),
                })
                .collect();
            return Ok((start, matches));
        }

        // Complete endpoint paths after HTTP commands
        if HTTP_COMMANDS.contains(&cmd) || cmd == "cd" {
            let prefix = if parts.len() > 1 && !line_up_to_cursor.ends_with(' ') {
                parts[1]
            } else {
                ""
            };
            let start = line_up_to_cursor.rfind(prefix).unwrap_or(pos);
            let matches: Vec<Pair> = state
                .endpoint_paths
                .iter()
                .filter(|path| path.starts_with(prefix))
                .map(|path| Pair {
                    display: path.clone(),
                    replacement: path.clone(),
                })
                .collect();
            return Ok((start, matches));
        }

        Ok((pos, Vec::new()))
    }
}

impl Hinter for ApiHelper {
    type Hint = String;
}

impl Highlighter for ApiHelper {}
impl Validator for ApiHelper {}
impl Helper for ApiHelper {}

pub struct ReplContext {
    config_dir: PathBuf,
    apis: HashMap<String, ApiConfig>,
    current_api: Option<String>,
    current_path: String,
    history: History,
    openapi_endpoints: HashMap<String, Vec<EndpointInfo>>,
    graphql_schemas: HashMap<String, GraphQLSchema>,
    completion_state: Arc<Mutex<CompletionState>>,
}

impl ReplContext {
    fn new(config_dir: PathBuf, completion_state: Arc<Mutex<CompletionState>>) -> Result<Self> {
        // Load all API configs
        let mut apis = HashMap::new();
        let configs = ApiConfig::load_all_from_dir(&config_dir)?;
        for config in configs {
            apis.insert(config.name.clone(), config);
        }

        // Load history
        let history_path = config_dir.join("history.json");
        let history = History::load(&history_path).unwrap_or_else(|_| History {
            entries: Vec::new(),
        });

        // Update completion state with API names
        {
            let mut state = completion_state.lock().unwrap();
            state.api_names = apis.keys().cloned().collect();
        }

        Ok(Self {
            config_dir,
            apis,
            current_api: None,
            current_path: "/".to_string(),
            history,
            openapi_endpoints: HashMap::new(),
            graphql_schemas: HashMap::new(),
            completion_state,
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
            "ls" => {
                if let Some(api) = &self.current_api.clone()
                    && self.is_graphql_api(api)
                {
                    self.list_graphql()?;
                } else {
                    self.list_endpoints()?;
                }
            }
            "get" | "GET" => self.http_request("GET", &parts[1..])?,
            "post" | "POST" => self.http_request("POST", &parts[1..])?,
            "put" | "PUT" => self.http_request("PUT", &parts[1..])?,
            "delete" | "DELETE" => self.http_request("DELETE", &parts[1..])?,
            "patch" | "PATCH" => self.http_request("PATCH", &parts[1..])?,
            "history" => self.show_history(),
            "config" => self.show_config()?,
            ":w" => self.save_config(&parts[1..])?,
            _ => println!(
                "{} Unknown command: {}. Type 'help' for available commands.",
                "✗".red(),
                parts[0]
            ),
        }

        Ok(true)
    }

    fn show_help(&self) {
        println!("{}", "Available commands:".bold());
        println!();
        println!("  {}  - Display this help message", "help".yellow());
        println!("  {}  - List available APIs", "apis".yellow());
        println!(
            "  {}  - Switch to an API (e.g., 'use github')",
            "use <api>".yellow()
        );
        println!(
            "  {}   - Change current path (e.g., 'cd /users')",
            "cd <path>".yellow()
        );
        println!("  {}   - Show current path", "pwd".yellow());
        println!(
            "  {}    - List available endpoints for current API",
            "ls".yellow()
        );
        println!();
        println!("  {}  - Make a GET request", "get <path>".yellow());
        println!(
            "  {} - Make a POST request with JSON body",
            "post <path> <json>".yellow()
        );
        println!(
            "  {}  - Make a PUT request with JSON body",
            "put <path> <json>".yellow()
        );
        println!(
            "  {} - Make a PATCH request with JSON body",
            "patch <path> <json>".yellow()
        );
        println!("  {} - Make a DELETE request", "delete <path>".yellow());
        println!();
        println!("  {} - Show request history", "history".yellow());
        println!("  {} - Show current API configuration", "config".yellow());
        println!(
            "  {} - Save current API config to a YAML file",
            ":w <file>".yellow()
        );
        println!("  {}  - Exit the shell", "exit".yellow());
        println!();
    }

    fn list_apis(&self) {
        if self.apis.is_empty() {
            println!(
                "{} No APIs configured. Add YAML config files to {}",
                "ℹ".blue(),
                self.config_dir.display()
            );
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
            return Err(anyhow!(
                "API '{}' not found. Use 'apis' to list available APIs.",
                api_name
            ));
        }

        self.current_api = Some(api_name.to_string());
        self.current_path = "/".to_string();
        println!(
            "{} Switched to API: {}",
            "✓".green(),
            api_name.cyan().bold()
        );

        // Load schema (OpenAPI or GraphQL) if applicable
        if self.is_graphql_api(api_name) {
            self.load_graphql_for_api(api_name);
        } else {
            self.load_openapi_for_api(api_name);
        }

        Ok(())
    }

    fn load_openapi_for_api(&mut self, api_name: &str) {
        if self.openapi_endpoints.contains_key(api_name) {
            self.update_completion_state();
            return;
        }

        let openapi_url = {
            let config = self.apis.get(api_name).unwrap();
            config.openapi.clone()
        };

        if let Some(url) = openapi_url {
            println!("{} Loading OpenAPI spec from {}...", "ℹ".blue(), url);
            match openapi::fetch_and_parse(&url) {
                Ok((_, endpoints)) => {
                    println!(
                        "{} Loaded {} endpoints from OpenAPI spec",
                        "✓".green(),
                        endpoints.len()
                    );
                    self.openapi_endpoints
                        .insert(api_name.to_string(), endpoints);
                    self.update_completion_state();
                }
                Err(e) => {
                    println!("{} Failed to load OpenAPI spec: {}", "✗".red(), e);
                }
            }
        }
    }

    fn is_graphql_api(&self, api_name: &str) -> bool {
        self.apis
            .get(api_name)
            .and_then(|c| c.graphql)
            .unwrap_or(false)
    }

    fn load_graphql_for_api(&mut self, api_name: &str) {
        if self.graphql_schemas.contains_key(api_name) {
            self.update_completion_state();
            return;
        }

        let (url, headers, auth) = {
            let config = self.apis.get(api_name).unwrap();
            (config.base_url.clone(), config.headers.clone(), config.auth.clone())
        };

        println!("{} Loading GraphQL schema from {}...", "ℹ".blue(), url);
        match crate::graphql::fetch_schema(&url, &headers, auth.as_ref()) {
            Ok(schema) => {
                let q_count = schema
                    .types
                    .get(&schema.query_type)
                    .map(|t| t.fields.len())
                    .unwrap_or(0);
                let m_count = schema
                    .mutation_type
                    .as_ref()
                    .and_then(|n| schema.types.get(n))
                    .map(|t| t.fields.len())
                    .unwrap_or(0);
                println!(
                    "{} Loaded GraphQL schema: {} queries, {} mutations",
                    "✓".green(),
                    q_count,
                    m_count
                );
                self.graphql_schemas.insert(api_name.to_string(), schema);
                self.update_completion_state();
            }
            Err(e) => {
                println!("{} Failed to load GraphQL schema: {}", "✗".red(), e);
                println!(
                    "{} Falling back to raw mode (no autocompletion or smart defaults)",
                    "ℹ".blue()
                );
            }
        }
    }

    fn update_completion_state(&self) {
        let mut paths = Vec::new();

        if let Some(api_name) = &self.current_api {
            // Add OpenAPI endpoint paths
            if let Some(endpoints) = self.openapi_endpoints.get(api_name) {
                for ep in endpoints {
                    paths.push(ep.path.clone());
                }
            }

            // Add configured endpoint paths
            if let Some(config) = self.apis.get(api_name) {
                for ep in &config.endpoints {
                    if !paths.contains(ep) {
                        paths.push(ep.clone());
                    }
                }
            }
        }

        let mut state = self.completion_state.lock().unwrap();
        state.endpoint_paths = paths;
        state.api_names = self.apis.keys().cloned().collect();
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
            let parts: Vec<&str> = self
                .current_path
                .trim_end_matches('/')
                .rsplitn(2, '/')
                .collect();
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
        let api_name = self
            .current_api
            .as_ref()
            .ok_or_else(|| anyhow!("No API selected. Use 'use <api>' first."))?;

        // Show OpenAPI endpoints if available
        if let Some(endpoints) = self.openapi_endpoints.get(api_name) {
            println!("{}", "Endpoints (from OpenAPI spec):".bold());
            for ep in endpoints {
                for method in &ep.methods {
                    let params: Vec<String> = method
                        .parameters
                        .iter()
                        .map(|p| {
                            if p.required {
                                format!("{}*", p.name)
                            } else {
                                p.name.clone()
                            }
                        })
                        .collect();

                    let param_str = if params.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", params.join(", "))
                    };

                    let summary_str = method
                        .summary
                        .as_ref()
                        .map(|s| format!(" - {}", s))
                        .unwrap_or_default();

                    println!(
                        "  {:6} {}{}{}",
                        method.method.yellow(),
                        ep.path.green(),
                        param_str.normal(),
                        summary_str.dimmed(),
                    );
                }
            }
            println!();
            return Ok(());
        }

        // Fall back to flat endpoint list
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

    fn list_graphql(&self) -> Result<()> {
        let api_name = self
            .current_api
            .as_ref()
            .ok_or_else(|| anyhow!("No API selected. Use 'use <api>' first."))?;

        let schema = self
            .graphql_schemas
            .get(api_name)
            .ok_or_else(|| anyhow!("GraphQL schema not loaded for {}", api_name))?;

        match crate::graphql::resolve_path(schema, &self.current_path) {
            Some(crate::graphql::Navigation::Root) => {
                // List queries and mutations
                if let Some(query_type) = schema.types.get(&schema.query_type) {
                    println!("{}", "Queries:".bold());
                    let display_limit = 15;
                    for (i, field) in query_type.fields.iter().enumerate() {
                        if i >= display_limit {
                            println!(
                                "  {} ({} more — use `ls -a` for all)",
                                "...".dimmed(),
                                query_type.fields.len() - display_limit
                            );
                            break;
                        }
                        let args: Vec<String> = field.args.iter().map(|a| a.name.clone()).collect();
                        let arg_str = if args.is_empty() {
                            String::new()
                        } else {
                            format!("({})", args.join(", "))
                        };
                        println!(
                            "  {}{} → {}",
                            field.name.green(),
                            arg_str.dimmed(),
                            field.type_ref.render().yellow()
                        );
                    }
                    println!();
                }

                if let Some(mut_name) = &schema.mutation_type
                    && let Some(mut_type) = schema.types.get(mut_name)
                {
                    println!("{}", "Mutations:".bold());
                    let display_limit = 15;
                    for (i, field) in mut_type.fields.iter().enumerate() {
                        if i >= display_limit {
                            println!(
                                "  {} ({} more — use `ls -a` for all)",
                                "...".dimmed(),
                                mut_type.fields.len() - display_limit
                            );
                            break;
                        }
                        println!("  {}", field.name.green());
                    }
                    println!();
                }
            }
            Some(crate::graphql::Navigation::Field { field, .. }) => {
                // Show args + return type fields
                if !field.args.is_empty() {
                    println!("{}", "Arguments:".bold());
                    for arg in &field.args {
                        println!(
                            "  {}: {}",
                            arg.name.cyan(),
                            arg.type_ref.render().yellow()
                        );
                    }
                    println!();
                }

                let return_type_name = field.type_ref.unwrap_name();
                if let Some(name) = return_type_name
                    && let Some(return_type) = schema.types.get(name)
                {
                    println!(
                        "{} ({})",
                        format!("Returns {}:", name).bold(),
                        format!("{:?}", return_type.kind).dimmed()
                    );
                    for f in &return_type.fields {
                        println!(
                            "  {} → {}",
                            f.name.green(),
                            f.type_ref.render().yellow()
                        );
                    }
                    println!();
                }
            }
            None => {
                println!(
                    "{} Invalid path: {}",
                    "✗".red(),
                    self.current_path
                );
            }
        }

        Ok(())
    }

    fn http_request(&mut self, method: &str, args: &[&str]) -> Result<()> {
        let api_name = self
            .current_api
            .as_ref()
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
            let status = entry
                .status_code
                .map(|s| format!("{}", s))
                .unwrap_or_else(|| "---".to_string());
            println!(
                "  {} {} {} ({})",
                entry.api.cyan(),
                entry.method.yellow(),
                entry.path.green(),
                status
            );
        }
        println!();
    }

    fn show_config(&self) -> Result<()> {
        let api_name = self
            .current_api
            .as_ref()
            .ok_or_else(|| anyhow!("No API selected. Use 'use <api>' first."))?;

        let config = self.apis.get(api_name).unwrap();

        println!("{}", "Current API Configuration:".bold());
        println!("  Name: {}", config.name.cyan());
        println!("  Base URL: {}", config.base_url.green());

        if let Some(openapi_url) = &config.openapi {
            println!("  OpenAPI: {}", openapi_url);
        }

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
        let api_name = self
            .current_api
            .as_ref()
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
    let completion_state = Arc::new(Mutex::new(CompletionState {
        endpoint_paths: Vec::new(),
        api_names: Vec::new(),
    }));

    let mut ctx = ReplContext::new(config_dir, Arc::clone(&completion_state))?;

    // If ad-hoc API provided, insert it and set as current
    if let Some(config) = adhoc_api {
        let name = config.name.clone();
        println!(
            "{} Connected to {} ({})",
            "✓".green(),
            name.cyan().bold(),
            config.base_url
        );
        ctx.apis.insert(name.clone(), config);
        ctx.current_api = Some(name.clone());

        // Load schema if configured
        if ctx.is_graphql_api(&name) {
            ctx.load_graphql_for_api(&name);
        } else {
            ctx.load_openapi_for_api(&name);
        }
    }

    let rl_config = Config::builder()
        .completion_type(CompletionType::List)
        .build();

    let helper = ApiHelper {
        state: Arc::clone(&completion_state),
    };

    let mut rl = Editor::with_config(rl_config)?;
    rl.set_helper(Some(helper));

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
