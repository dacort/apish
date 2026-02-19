use anyhow::{Result, anyhow};
use oas3::spec::{ObjectOrReference, ParameterIn};
use reqwest::blocking::Client;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct EndpointInfo {
    pub path: String,
    pub methods: Vec<MethodInfo>,
}

#[derive(Debug, Clone)]
pub struct MethodInfo {
    pub method: String,
    pub summary: Option<String>,
    pub parameters: Vec<ParamInfo>,
}

#[derive(Debug, Clone)]
pub struct ParamInfo {
    pub name: String,
    pub location: String,
    pub required: bool,
}

/// Parse an OpenAPI spec from a string (tries JSON then YAML).
/// Returns (server_url, endpoints).
pub fn parse_spec(content: &str) -> Result<(Option<String>, Vec<EndpointInfo>)> {
    // Try JSON first, then YAML
    let spec: oas3::Spec = oas3::from_json(content)
        .or_else(|_| oas3::from_yaml(content))
        .map_err(|e| anyhow!("Failed to parse OpenAPI spec: {}", e))?;

    let server_url = spec.servers.first().map(|s| s.url.clone());

    let mut endpoints = Vec::new();

    if let Some(paths) = &spec.paths {
        for (path_str, path_item) in paths {
            let mut methods = Vec::new();

            let ops: Vec<(&str, Option<&oas3::spec::Operation>)> = vec![
                ("GET", path_item.get.as_ref()),
                ("POST", path_item.post.as_ref()),
                ("PUT", path_item.put.as_ref()),
                ("DELETE", path_item.delete.as_ref()),
                ("PATCH", path_item.patch.as_ref()),
                ("HEAD", path_item.head.as_ref()),
                ("OPTIONS", path_item.options.as_ref()),
            ];

            for (method_name, maybe_op) in ops {
                if let Some(operation) = maybe_op {
                    let summary = operation.summary.clone();

                    let mut parameters = Vec::new();

                    // Collect path-level parameters
                    for param_ref in &path_item.parameters {
                        if let Some(param) = resolve_parameter(param_ref, &spec) {
                            parameters.push(param);
                        }
                    }

                    // Collect operation-level parameters (may override path-level)
                    for param_ref in &operation.parameters {
                        if let Some(param) = resolve_parameter(param_ref, &spec) {
                            parameters.retain(|p: &ParamInfo| {
                                !(p.name == param.name && p.location == param.location)
                            });
                            parameters.push(param);
                        }
                    }

                    methods.push(MethodInfo {
                        method: method_name.to_string(),
                        summary,
                        parameters,
                    });
                }
            }

            if !methods.is_empty() {
                endpoints.push(EndpointInfo {
                    path: path_str.clone(),
                    methods,
                });
            }
        }
    }

    // Sort endpoints by path for consistent display
    endpoints.sort_by(|a, b| a.path.cmp(&b.path));

    Ok((server_url, endpoints))
}

fn resolve_parameter(
    param_ref: &ObjectOrReference<oas3::spec::Parameter>,
    spec: &oas3::Spec,
) -> Option<ParamInfo> {
    let param = param_ref.resolve(spec).ok()?;
    Some(ParamInfo {
        name: param.name.clone(),
        location: match param.location {
            ParameterIn::Query => "query".to_string(),
            ParameterIn::Header => "header".to_string(),
            ParameterIn::Path => "path".to_string(),
            ParameterIn::Cookie => "cookie".to_string(),
        },
        required: param.required.unwrap_or(false),
    })
}

/// Fetch an OpenAPI spec from a URL and parse it.
pub fn fetch_and_parse(url: &str) -> Result<(Option<String>, Vec<EndpointInfo>)> {
    let client = Client::builder().timeout(Duration::from_secs(15)).build()?;

    let response = client.get(url).send()?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "Failed to fetch OpenAPI spec: HTTP {}",
            response.status()
        ));
    }

    let content = response.text()?;
    parse_spec(&content)
}

/// Quick check if content looks like an OpenAPI spec.
pub fn is_openapi_spec(content: &str) -> bool {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(content)
        && json.get("openapi").is_some()
    {
        return true;
    }

    if let Ok(serde_yaml::Value::Mapping(map)) = serde_yaml::from_str::<serde_yaml::Value>(content)
    {
        return map.contains_key(serde_yaml::Value::String("openapi".to_string()));
    }

    false
}
