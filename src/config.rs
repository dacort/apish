use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub endpoints: Vec<String>,
    #[serde(default)]
    pub openapi: Option<String>,
    #[serde(default)]
    pub graphql: Option<bool>,
    #[serde(default)]
    pub auth: Option<AuthConfig>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AuthConfig {
    Bearer {
        token: String,
    },
    ApiKey {
        name: String,
        value: String,
        location: KeyLocation,
    },
    Basic {
        username: String,
        password: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyLocation {
    Header,
    Query,
}

impl ApiConfig {
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .context(format!("Failed to read config from {:?}", path))?;

        serde_yaml::from_str(&content).context("Failed to parse YAML config")
    }

    pub fn load_all_from_dir(dir: &Path) -> Result<Vec<Self>> {
        let mut configs = Vec::new();

        if !dir.exists() {
            return Ok(configs);
        }

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) == Some("yaml")
                || path.extension().and_then(|s| s.to_str()) == Some("yml")
            {
                match Self::load_from_file(&path) {
                    Ok(config) => configs.push(config),
                    Err(e) => eprintln!("Warning: Failed to load {:?}: {}", path, e),
                }
            }
        }

        Ok(configs)
    }

    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        let content = serde_yaml::to_string(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct History {
    pub entries: Vec<HistoryEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub api: String,
    pub method: String,
    pub path: String,
    pub timestamp: String,
    pub status_code: Option<u16>,
}

impl History {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                entries: Vec::new(),
            });
        }

        let content = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    pub fn add_entry(
        &mut self,
        api: String,
        method: String,
        path: String,
        status_code: Option<u16>,
    ) {
        self.entries.push(HistoryEntry {
            api,
            method,
            path,
            timestamp: chrono::Utc::now().to_rfc3339(),
            status_code,
        });

        // Keep only last 1000 entries
        if self.entries.len() > 1000 {
            self.entries.drain(0..self.entries.len() - 1000);
        }
    }

    pub fn get_paths_for_api(&self, api: &str) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| e.api == api)
            .map(|e| e.path.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect()
    }
}
