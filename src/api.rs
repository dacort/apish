use crate::config::{ApiConfig, AuthConfig, KeyLocation};
use anyhow::{Result, anyhow};
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use std::time::Duration;

pub struct ApiClient {
    client: Client,
    config: ApiConfig,
}

impl ApiClient {
    pub fn new(config: ApiConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;

        Ok(Self { client, config })
    }

    pub fn config(&self) -> &ApiConfig {
        &self.config
    }

    fn build_request(&self, method: &str, path: &str) -> Result<RequestBuilder> {
        let url = if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{}{}", self.config.base_url.trim_end_matches('/'), path)
        };

        let mut req = match method.to_uppercase().as_str() {
            "GET" => self.client.get(&url),
            "POST" => self.client.post(&url),
            "PUT" => self.client.put(&url),
            "DELETE" => self.client.delete(&url),
            "PATCH" => self.client.patch(&url),
            _ => return Err(anyhow!("Unsupported HTTP method: {}", method)),
        };

        // Add configured headers
        for (key, value) in &self.config.headers {
            req = req.header(key, value);
        }

        // Add auth
        if let Some(auth) = &self.config.auth {
            req = match auth {
                AuthConfig::Bearer { token } => req.bearer_auth(token),
                AuthConfig::ApiKey { name, value, location } => match location {
                    KeyLocation::Header => req.header(name, value),
                    KeyLocation::Query => req.query(&[(name, value)]),
                },
                AuthConfig::Basic { username, password } => req.basic_auth(username, Some(password)),
            };
        }

        Ok(req)
    }

    pub fn request(&self, method: &str, path: &str, body: Option<&str>) -> Result<ApiResponse> {
        let mut req = self.build_request(method, path)?;

        // Add body for POST/PUT/PATCH
        if let Some(body_str) = body {
            if !body_str.is_empty() {
                req = req.header("Content-Type", "application/json").body(body_str.to_string());
            }
        }

        let response = req.send()?;
        let status = response.status();
        let headers = response.headers().clone();

        let body_text = response.text()?;
        let body_json = serde_json::from_str::<Value>(&body_text).ok();

        Ok(ApiResponse {
            status_code: status.as_u16(),
            status_text: status.to_string(),
            headers,
            body: body_json,
            raw_body: body_text,
        })
    }
}

pub struct ApiResponse {
    pub status_code: u16,
    pub status_text: String,
    pub headers: HeaderMap,
    pub body: Option<Value>,
    pub raw_body: String,
}

impl ApiResponse {
    pub fn print(&self) {
        use colored::Colorize;

        // Status line
        let status_color = if self.status_code < 300 {
            "green"
        } else if self.status_code < 400 {
            "yellow"
        } else {
            "red"
        };

        println!("{} {}",
            format!("{}", self.status_code).color(status_color).bold(),
            self.status_text.color(status_color)
        );

        // Pretty-print JSON body
        if let Some(json) = &self.body {
            println!();
            println!("{}", serde_json::to_string_pretty(json).unwrap());
        } else if !self.raw_body.is_empty() {
            println!();
            println!("{}", self.raw_body);
        }

        println!();
    }
}
