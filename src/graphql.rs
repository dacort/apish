use crate::config::{AuthConfig, KeyLocation};
use anyhow::{Context, Result, anyhow};
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::runtime::Builder as TokioRuntimeBuilder;
use wreq::Client as EmulatedClient;
use wreq_util::Emulation as BrowserEmulation;

#[derive(Debug, Clone)]
pub struct GraphQLSchema {
    pub query_type: String,
    pub mutation_type: Option<String>,
    pub types: HashMap<String, TypeDef>,
}

#[derive(Debug, Clone)]
pub struct TypeDef {
    pub name: String,
    pub kind: TypeKind,
    pub fields: Vec<FieldDef>,
    pub input_fields: Vec<InputFieldDef>,
    pub enum_values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeKind {
    Scalar,
    Object,
    Interface,
    Union,
    Enum,
    InputObject,
    List,
    NonNull,
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: String,
    pub description: Option<String>,
    pub args: Vec<ArgDef>,
    pub type_ref: TypeRef,
}

#[derive(Debug, Clone)]
pub struct InputFieldDef {
    pub name: String,
    pub type_ref: TypeRef,
    pub default_value: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ArgDef {
    pub name: String,
    pub type_ref: TypeRef,
    pub default_value: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TypeRef {
    pub kind: TypeKind,
    pub name: Option<String>,
    pub of_type: Option<Box<TypeRef>>,
}

impl TypeRef {
    /// Returns the named type at the bottom of any LIST/NON_NULL wrappers.
    pub fn unwrap_name(&self) -> Option<&str> {
        match (&self.kind, &self.of_type) {
            (TypeKind::List | TypeKind::NonNull, Some(inner)) => inner.unwrap_name(),
            _ => self.name.as_deref(),
        }
    }

    /// Renders a type ref as a GraphQL type string, e.g. "[Product!]!"
    pub fn render(&self) -> String {
        match self.kind {
            TypeKind::NonNull => {
                let inner = self
                    .of_type
                    .as_ref()
                    .map(|t| t.render())
                    .unwrap_or_default();
                format!("{}!", inner)
            }
            TypeKind::List => {
                let inner = self
                    .of_type
                    .as_ref()
                    .map(|t| t.render())
                    .unwrap_or_default();
                format!("[{}]", inner)
            }
            _ => self.name.clone().unwrap_or_else(|| "?".to_string()),
        }
    }
}

// ----- Raw introspection JSON shapes (for serde) -----

#[derive(Debug, Deserialize)]
struct IntrospectionRoot {
    data: IntrospectionData,
}

#[derive(Debug, Deserialize)]
struct IntrospectionData {
    #[serde(rename = "__schema")]
    schema: IntrospectionSchema,
}

#[derive(Debug, Deserialize)]
struct IntrospectionSchema {
    #[serde(rename = "queryType")]
    query_type: IntrospectionTypeName,
    #[serde(rename = "mutationType")]
    mutation_type: Option<IntrospectionTypeName>,
    types: Vec<IntrospectionType>,
}

#[derive(Debug, Deserialize)]
struct IntrospectionTypeName {
    name: String,
}

#[derive(Debug, Deserialize)]
struct IntrospectionType {
    kind: String,
    name: Option<String>,
    fields: Option<Vec<IntrospectionField>>,
    #[serde(rename = "inputFields")]
    input_fields: Option<Vec<IntrospectionInputField>>,
    #[serde(rename = "enumValues")]
    enum_values: Option<Vec<IntrospectionEnumValue>>,
}

#[derive(Debug, Deserialize)]
struct IntrospectionField {
    name: String,
    description: Option<String>,
    args: Vec<IntrospectionInputField>,
    #[serde(rename = "type")]
    type_ref: IntrospectionTypeRef,
}

#[derive(Debug, Deserialize)]
struct IntrospectionInputField {
    name: String,
    #[serde(rename = "type")]
    type_ref: IntrospectionTypeRef,
    #[serde(rename = "defaultValue")]
    default_value: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IntrospectionEnumValue {
    name: String,
}

#[derive(Debug, Deserialize)]
struct IntrospectionTypeRef {
    kind: String,
    name: Option<String>,
    #[serde(rename = "ofType")]
    of_type: Option<Box<IntrospectionTypeRef>>,
}

// ----- Conversion -----

fn parse_kind(s: &str) -> Result<TypeKind> {
    Ok(match s {
        "SCALAR" => TypeKind::Scalar,
        "OBJECT" => TypeKind::Object,
        "INTERFACE" => TypeKind::Interface,
        "UNION" => TypeKind::Union,
        "ENUM" => TypeKind::Enum,
        "INPUT_OBJECT" => TypeKind::InputObject,
        "LIST" => TypeKind::List,
        "NON_NULL" => TypeKind::NonNull,
        other => return Err(anyhow!("unknown type kind: {}", other)),
    })
}

fn convert_type_ref(t: &IntrospectionTypeRef) -> Result<TypeRef> {
    Ok(TypeRef {
        kind: parse_kind(&t.kind)?,
        name: t.name.clone(),
        of_type: match &t.of_type {
            Some(inner) => Some(Box::new(convert_type_ref(inner)?)),
            None => None,
        },
    })
}

fn convert_arg(a: &IntrospectionInputField) -> Result<ArgDef> {
    Ok(ArgDef {
        name: a.name.clone(),
        type_ref: convert_type_ref(&a.type_ref)?,
        default_value: a.default_value.clone(),
    })
}

fn convert_input_field(f: &IntrospectionInputField) -> Result<InputFieldDef> {
    Ok(InputFieldDef {
        name: f.name.clone(),
        type_ref: convert_type_ref(&f.type_ref)?,
        default_value: f.default_value.clone(),
    })
}

fn convert_field(f: &IntrospectionField) -> Result<FieldDef> {
    Ok(FieldDef {
        name: f.name.clone(),
        description: f.description.clone(),
        args: f.args.iter().map(convert_arg).collect::<Result<Vec<_>>>()?,
        type_ref: convert_type_ref(&f.type_ref)?,
    })
}

pub fn parse_introspection(body: &str) -> Result<GraphQLSchema> {
    let root: IntrospectionRoot =
        serde_json::from_str(body).map_err(|e| anyhow!("invalid introspection JSON: {}", e))?;

    let mut types = HashMap::new();
    for t in &root.data.schema.types {
        let name = match &t.name {
            Some(n) => n.clone(),
            None => continue, // anonymous LIST/NON_NULL wrappers in the type list
        };
        let kind = parse_kind(&t.kind)?;
        let fields = t
            .fields
            .as_ref()
            .map(|fs| fs.iter().map(convert_field).collect::<Result<Vec<_>>>())
            .transpose()?
            .unwrap_or_default();
        let input_fields = t
            .input_fields
            .as_ref()
            .map(|fs| {
                fs.iter()
                    .map(convert_input_field)
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();
        let enum_values = t
            .enum_values
            .as_ref()
            .map(|vs| vs.iter().map(|v| v.name.clone()).collect())
            .unwrap_or_default();

        types.insert(
            name.clone(),
            TypeDef {
                name,
                kind,
                fields,
                input_fields,
                enum_values,
            },
        );
    }

    Ok(GraphQLSchema {
        query_type: root.data.schema.query_type.name,
        mutation_type: root.data.schema.mutation_type.map(|m| m.name),
        types,
    })
}

const DEFAULT_INTROSPECTION_TYPE_DEPTH: usize = 6;
const SHALLOW_INTROSPECTION_TYPE_DEPTH: usize = 2;

fn build_type_ref_query(depth: usize) -> String {
    if depth == 0 {
        return "kind name".to_string();
    }

    format!(
        "kind name ofType {{ {} }}",
        build_type_ref_query(depth.saturating_sub(1))
    )
}

fn build_introspection_query(of_type_depth: usize) -> String {
    let type_ref = build_type_ref_query(of_type_depth);
    format!(
        r#"{{
  __schema {{
    queryType {{ name }}
    mutationType {{ name }}
    types {{
      kind
      name
      fields(includeDeprecated: true) {{
        name
        description
        args {{
          name
          type {{ {type_ref} }}
          defaultValue
        }}
        type {{ {type_ref} }}
      }}
      inputFields {{
        name
        type {{ {type_ref} }}
        defaultValue
      }}
      enumValues(includeDeprecated: true) {{ name }}
    }}
  }}
}}"#
    )
}

fn is_query_depth_limit_error(status: u16, body_text: &str) -> bool {
    status == 413
        && (body_text.contains("Query depth limit exceeded")
            || body_text.contains("GCDN_QUERY_DEPTH_LIMIT"))
}

#[derive(Debug)]
struct GraphQLHttpResponse {
    status: u16,
    body_text: String,
    content_type: Option<String>,
}

fn waf_fallback_cache() -> &'static Mutex<HashSet<String>> {
    static CACHE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashSet::new()))
}

fn should_use_waf_fallback(url: &str) -> bool {
    waf_fallback_cache()
        .lock()
        .expect("waf fallback cache poisoned")
        .contains(url)
}

fn remember_waf_fallback(url: &str) {
    waf_fallback_cache()
        .lock()
        .expect("waf fallback cache poisoned")
        .insert(url.to_string());
}

fn post_graphql_json(
    url: &str,
    headers: &HashMap<String, String>,
    auth: Option<&AuthConfig>,
    payload: &Value,
) -> Result<GraphQLHttpResponse> {
    if should_use_waf_fallback(url) {
        return post_graphql_with_wreq(url, headers, auth, payload).with_context(|| {
            format!(
                "GraphQL POST to {} is using cached browser-emulated fallback",
                url
            )
        });
    }

    match post_graphql_with_reqwest(url, headers, auth, payload) {
        Ok(resp)
            if looks_like_waf_block(resp.status, resp.content_type.as_deref(), &resp.body_text) =>
        {
            remember_waf_fallback(url);
            post_graphql_with_wreq(url, headers, auth, payload).with_context(|| {
                format!(
                    "GraphQL POST to {} was blocked by an upstream WAF when using reqwest",
                    url
                )
            })
        }
        Ok(resp) => Ok(resp),
        Err(reqwest_err) => post_graphql_with_wreq(url, headers, auth, payload).with_context(|| {
            format!(
                "GraphQL POST via reqwest failed: {}. Browser-emulated Rust fallback also failed",
                reqwest_err
            )
        }),
    }
}

fn post_graphql_with_reqwest(
    url: &str,
    headers: &HashMap<String, String>,
    auth: Option<&AuthConfig>,
    payload: &Value,
) -> Result<GraphQLHttpResponse> {
    let client = Client::builder().timeout(Duration::from_secs(30)).build()?;

    let mut req = client.post(url).header("Content-Type", "application/json");

    for (k, v) in headers {
        req = req.header(k, v);
    }

    if let Some(auth) = auth {
        req = match auth {
            AuthConfig::Bearer { token } => req.bearer_auth(token),
            AuthConfig::ApiKey {
                name,
                value,
                location,
            } => match location {
                KeyLocation::Header => req.header(name, value),
                KeyLocation::Query => req.query(&[(name, value)]),
            },
            AuthConfig::Basic { username, password } => req.basic_auth(username, Some(password)),
        };
    }

    let response = req.body(serde_json::to_string(payload)?).send()?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string());
    let body_text = response.text()?;

    Ok(GraphQLHttpResponse {
        status,
        body_text,
        content_type,
    })
}

fn looks_like_waf_block(status: u16, content_type: Option<&str>, body: &str) -> bool {
    if status != 403 && status != 406 && status != 429 && status != 503 {
        return false;
    }

    let body_lower = body.to_ascii_lowercase();
    let is_html = content_type
        .map(|ct| ct.to_ascii_lowercase().contains("text/html"))
        .unwrap_or(false)
        || body_lower.contains("<html");

    is_html
        && (body_lower.contains("access denied")
            || body_lower.contains("reference #")
            || body_lower.contains("errors.edgesuite.net"))
}

fn post_graphql_with_wreq(
    url: &str,
    headers: &HashMap<String, String>,
    auth: Option<&AuthConfig>,
    payload: &Value,
) -> Result<GraphQLHttpResponse> {
    let mut target_url = url::Url::parse(url)?;
    let mut header_auth: Option<(String, String)> = None;
    let mut basic_auth: Option<(String, String)> = None;

    if let Some(auth) = auth {
        match auth {
            AuthConfig::Bearer { token } => {
                header_auth = Some(("authorization".to_string(), format!("Bearer {}", token)));
            }
            AuthConfig::ApiKey {
                name,
                value,
                location,
            } => match location {
                KeyLocation::Header => header_auth = Some((name.clone(), value.clone())),
                KeyLocation::Query => {
                    target_url.query_pairs_mut().append_pair(name, value);
                }
            },
            AuthConfig::Basic { username, password } => {
                basic_auth = Some((username.clone(), password.clone()));
            }
        }
    }

    let runtime = TokioRuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime for GraphQL WAF fallback")?;

    runtime.block_on(async {
        let client = EmulatedClient::builder()
            .emulation(BrowserEmulation::Safari26)
            .build()
            .context("failed to build browser-emulated HTTP client")?;

        let mut req = client
            .post(target_url.as_str())
            .header("Content-Type", "application/json");

        for (k, v) in headers {
            req = req.header(k, v);
        }

        if let Some((name, value)) = &header_auth {
            req = req.header(name, value);
        }
        if let Some((username, password)) = &basic_auth {
            req = req.basic_auth(username, Some(password));
        }

        let response = req.body(serde_json::to_vec(payload)?).send().await?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string());
        let body_text = response.text().await?;

        Ok(GraphQLHttpResponse {
            status,
            body_text,
            content_type,
        })
    })
}

/// Fetches the schema from a GraphQL endpoint by POSTing the introspection query.
pub fn fetch_schema(
    url: &str,
    headers: &HashMap<String, String>,
    auth: Option<&AuthConfig>,
) -> Result<GraphQLSchema> {
    let full_payload = serde_json::json!({
        "query": build_introspection_query(DEFAULT_INTROSPECTION_TYPE_DEPTH),
    });
    let response = post_graphql_json(url, headers, auth, &full_payload)?;
    let response = if is_query_depth_limit_error(response.status, &response.body_text) {
        let shallow_payload = serde_json::json!({
            "query": build_introspection_query(SHALLOW_INTROSPECTION_TYPE_DEPTH),
        });
        post_graphql_json(url, headers, auth, &shallow_payload)?
    } else {
        response
    };
    if response.status < 200 || response.status >= 300 {
        return Err(anyhow!(
            "introspection failed with status {}",
            response.status
        ));
    }

    parse_introspection(&response.body_text)
}

/// Where the user currently "is" within a GraphQL schema.
#[derive(Debug, Clone)]
pub enum Navigation<'a> {
    Root,
    Field {
        /// The name of the type containing this field (e.g. "Query", "Products").
        type_name: String,
        /// The field name (e.g. "products", "items").
        field_name: String,
        /// The field definition itself, for arg lookup.
        field: &'a FieldDef,
    },
}

/// Resolves a slash-separated path against the schema, starting at Query.
/// Returns None if any path segment doesn't exist as a field on the current type.
pub fn resolve_path<'a>(schema: &'a GraphQLSchema, path: &str) -> Option<Navigation<'a>> {
    let segments: Vec<&str> = path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    if segments.is_empty() {
        return Some(Navigation::Root);
    }

    let mut current_type_name = schema.query_type.clone();
    let mut last_field: Option<&FieldDef> = None;

    for (i, segment) in segments.iter().enumerate() {
        let type_def = schema.types.get(&current_type_name)?;
        let field = type_def.fields.iter().find(|f| f.name == *segment)?;

        if i == segments.len() - 1 {
            return Some(Navigation::Field {
                type_name: current_type_name.clone(),
                field_name: segment.to_string(),
                field,
            });
        }

        // Descend into the field's return type.
        let next_type_name = field.type_ref.unwrap_name()?.to_string();
        current_type_name = next_type_name;
        last_field = Some(field);
    }

    // Unreachable: loop always returns on the last segment.
    let _ = last_field;
    None
}

/// Quick check: does this URL respond to a GraphQL introspection query with a valid schema?
pub fn detect_graphql(url: &str) -> bool {
    let payload = serde_json::json!({
        "query": "{ __schema { queryType { name } } }",
    });
    let response = match post_graphql_json(url, &HashMap::new(), None, &payload) {
        Ok(resp) => resp,
        Err(_) => return false,
    };
    if response.status < 200 || response.status >= 300 {
        return false;
    }
    // Look for the structural marker
    response.body_text.contains("\"__schema\"") && response.body_text.contains("\"queryType\"")
}

/// Parses CLI-style key:value args (with dot notation for nesting) into a JSON object.
///
/// Examples:
///   `search:milk`                  → `{"search": "milk"}`
///   `pageSize:3`                   → `{"pageSize": 3}`
///   `filter.store_code.eq:"TJ"`    → `{"filter": {"store_code": {"eq": "TJ"}}}`
///
/// Type inference (when value is unquoted):
///   - bare integer → JSON number
///   - bare float   → JSON number
///   - `true`/`false` → JSON bool
///   - `null` → JSON null
///   - everything else → JSON string
pub fn parse_args(tokens: &[&str]) -> Result<Map<String, Value>> {
    let mut root = Map::new();
    for token in tokens {
        let (path, raw_value) = token
            .split_once(':')
            .ok_or_else(|| anyhow!("argument missing ':' separator: {}", token))?;
        let value = parse_scalar_value(raw_value);
        insert_nested(&mut root, path, value)?;
    }
    Ok(root)
}

fn parse_scalar_value(raw: &str) -> Value {
    // Quoted string
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        return Value::String(raw[1..raw.len() - 1].to_string());
    }
    // Bool
    if raw == "true" {
        return Value::Bool(true);
    }
    if raw == "false" {
        return Value::Bool(false);
    }
    // Null
    if raw == "null" {
        return Value::Null;
    }
    // Int / Float
    if let Ok(n) = raw.parse::<i64>() {
        return Value::Number(n.into());
    }
    if let Ok(f) = raw.parse::<f64>()
        && let Some(n) = serde_json::Number::from_f64(f)
    {
        return Value::Number(n);
    }
    // Fall through: bare string
    Value::String(raw.to_string())
}

fn coerce_value_for_type(
    value: &Value,
    type_ref: &TypeRef,
    schema: &GraphQLSchema,
) -> Result<Value> {
    match type_ref.kind {
        TypeKind::NonNull => {
            let inner = type_ref
                .of_type
                .as_ref()
                .ok_or_else(|| anyhow!("non-null type missing inner type"))?;
            if value.is_null() {
                return Err(anyhow!("expected non-null value"));
            }
            coerce_value_for_type(value, inner, schema)
        }
        TypeKind::List => {
            let inner = type_ref
                .of_type
                .as_ref()
                .ok_or_else(|| anyhow!("list type missing inner type"))?;
            match value {
                Value::Array(items) => Ok(Value::Array(
                    items
                        .iter()
                        .map(|item| coerce_value_for_type(item, inner, schema))
                        .collect::<Result<Vec<_>>>()?,
                )),
                _ => Err(anyhow!("expected a list value")),
            }
        }
        _ => {
            let type_name = type_ref.unwrap_name().unwrap_or_default();
            match schema.types.get(type_name) {
                Some(type_def) if matches!(type_def.kind, TypeKind::InputObject) => {
                    coerce_input_object(value, type_def, schema)
                }
                Some(type_def) if matches!(type_def.kind, TypeKind::Enum) => {
                    coerce_enum_value(value, type_def)
                }
                _ => coerce_scalar_value(value, type_name),
            }
        }
    }
}

fn coerce_input_object(value: &Value, type_def: &TypeDef, schema: &GraphQLSchema) -> Result<Value> {
    let obj = value
        .as_object()
        .ok_or_else(|| anyhow!("expected input object {}", type_def.name))?;

    let mut out = Map::new();
    for (key, raw_value) in obj {
        let field_def = type_def
            .input_fields
            .iter()
            .find(|f| f.name == *key)
            .ok_or_else(|| anyhow!("'{}' is not a field on {}", key, type_def.name))?;
        out.insert(
            key.clone(),
            coerce_value_for_type(raw_value, &field_def.type_ref, schema)
                .map_err(|e| anyhow!("invalid value for {}.{}: {}", type_def.name, key, e))?,
        );
    }
    Ok(Value::Object(out))
}

fn coerce_enum_value(value: &Value, type_def: &TypeDef) -> Result<Value> {
    let s = value
        .as_str()
        .ok_or_else(|| anyhow!("expected enum value for {}", type_def.name))?;
    if type_def.enum_values.iter().any(|v| v == s) {
        Ok(Value::String(s.to_string()))
    } else {
        Err(anyhow!("invalid enum value '{}' for {}", s, type_def.name))
    }
}

fn coerce_scalar_value(value: &Value, type_name: &str) -> Result<Value> {
    match type_name {
        "String" | "ID" => match value {
            Value::String(s) => Ok(Value::String(s.clone())),
            Value::Number(n) => Ok(Value::String(n.to_string())),
            Value::Bool(b) => Ok(Value::String(b.to_string())),
            Value::Null => Ok(Value::Null),
            _ => Err(anyhow!("expected scalar {}", type_name)),
        },
        "Int" => match value {
            Value::Number(n) if n.is_i64() || n.is_u64() => Ok(Value::Number(n.clone())),
            Value::String(s) => {
                let parsed = s.parse::<i64>().map_err(|_| anyhow!("expected integer"))?;
                Ok(Value::Number(parsed.into()))
            }
            _ => Err(anyhow!("expected integer")),
        },
        "Float" => match value {
            Value::Number(n) => Ok(Value::Number(n.clone())),
            Value::String(s) => {
                let parsed = s.parse::<f64>().map_err(|_| anyhow!("expected float"))?;
                let num =
                    serde_json::Number::from_f64(parsed).ok_or_else(|| anyhow!("invalid float"))?;
                Ok(Value::Number(num))
            }
            _ => Err(anyhow!("expected float")),
        },
        "Boolean" => match value {
            Value::Bool(b) => Ok(Value::Bool(*b)),
            Value::String(s) if s == "true" => Ok(Value::Bool(true)),
            Value::String(s) if s == "false" => Ok(Value::Bool(false)),
            _ => Err(anyhow!("expected boolean")),
        },
        _ => Ok(value.clone()),
    }
}

fn insert_nested(root: &mut Map<String, Value>, path: &str, value: Value) -> Result<()> {
    let segments: Vec<&str> = path.split('.').collect();
    if segments.is_empty() {
        return Err(anyhow!("empty argument path"));
    }

    let mut current = root;
    for (i, segment) in segments.iter().enumerate() {
        if i == segments.len() - 1 {
            current.insert(segment.to_string(), value);
            return Ok(());
        }
        let entry = current
            .entry(segment.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        match entry {
            Value::Object(obj) => current = obj,
            _ => {
                return Err(anyhow!(
                    "conflicting types at argument path segment '{}'",
                    segment
                ));
            }
        }
    }
    Ok(())
}

const MAX_SELECTION_DEPTH: usize = 2;

fn is_scalar_kind(k: &TypeKind) -> bool {
    matches!(k, TypeKind::Scalar | TypeKind::Enum)
}

fn field_has_required_arg(field: &FieldDef) -> bool {
    field
        .args
        .iter()
        .any(|a| matches!(a.type_ref.kind, TypeKind::NonNull) && a.default_value.is_none())
}

fn input_field_has_required_arg(type_ref: &TypeRef, schema: &GraphQLSchema) -> bool {
    match type_ref.kind {
        TypeKind::NonNull => {
            let inner = type_ref.of_type.as_ref();
            if let Some(inner) = inner {
                if let Some(name) = inner.unwrap_name()
                    && let Some(type_def) = schema.types.get(name)
                    && matches!(type_def.kind, TypeKind::InputObject)
                {
                    return type_def.input_fields.iter().any(|f| {
                        matches!(f.type_ref.kind, TypeKind::NonNull) && f.default_value.is_none()
                    });
                }
            }
            true
        }
        _ => false,
    }
}

/// Walks through LIST/NON_NULL wrappers to return the innermost TypeRef.
fn unwrap_type_ref(t: &TypeRef) -> &TypeRef {
    match (&t.kind, &t.of_type) {
        (TypeKind::List | TypeKind::NonNull, Some(inner)) => unwrap_type_ref(inner),
        _ => t,
    }
}

/// Builds a GraphQL selection set string for the given type, automatically
/// picking all scalar fields plus one level of nested object scalars (up to MAX_SELECTION_DEPTH).
/// Returns a string like "{ items { sku item_title } total_count }".
pub fn smart_select_fields(schema: &GraphQLSchema, type_def: &TypeDef, depth: usize) -> String {
    let mut parts: Vec<String> = Vec::new();

    for field in &type_def.fields {
        if field_has_required_arg(field) {
            continue;
        }

        // Resolve the innermost type ref (unwrapping LIST/NON_NULL).
        let inner = unwrap_type_ref(&field.type_ref);
        let inner_name = inner.name.as_deref();
        let inner_type = inner_name.and_then(|n| schema.types.get(n));

        // Prefer the schema's type definition when available so we know the actual kind;
        // otherwise fall back to the type_ref's own kind (which is populated for scalars
        // and enums in most introspection responses).
        let (kind, nested_type) = match inner_type {
            Some(t) => (t.kind.clone(), Some(t)),
            None => (inner.kind.clone(), None),
        };

        if is_scalar_kind(&kind) {
            parts.push(field.name.clone());
        } else if matches!(kind, TypeKind::Object | TypeKind::Interface)
            && depth < MAX_SELECTION_DEPTH
        {
            if let Some(t) = nested_type {
                let nested = smart_select_fields(schema, t, depth + 1);
                if !nested.is_empty() && nested != "{ }" {
                    parts.push(format!("{} {}", field.name, nested));
                }
            }
        }
        // Otherwise: skip union, input_object, too-deep nesting, or unknown types.
    }

    if parts.is_empty() {
        return String::new();
    }
    format!("{{ {} }}", parts.join(" "))
}

/// Parses the shorthand `--fields` syntax into a GraphQL selection set string.
///
/// Input:  `items{sku,item_title},total_count`
/// Output: `{ items { sku item_title } total_count }`
///
/// Whitespace inside the input is ignored. Curly braces nest, commas separate.
pub fn parse_field_selection(input: &str) -> Result<String> {
    let cleaned: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    let mut chars = cleaned.chars().peekable();
    let body = parse_selection_body(&mut chars)?;
    Ok(format!("{{ {} }}", body))
}

fn parse_selection_body<I>(chars: &mut std::iter::Peekable<I>) -> Result<String>
where
    I: Iterator<Item = char>,
{
    let mut parts: Vec<String> = Vec::new();
    let mut current_name = String::new();

    while let Some(&c) = chars.peek() {
        match c {
            '{' => {
                chars.next();
                let inner = parse_selection_body(chars)?;
                let name = std::mem::take(&mut current_name);
                if name.is_empty() {
                    return Err(anyhow!("'{{' without a preceding field name"));
                }
                parts.push(format!("{} {{ {} }}", name, inner));
            }
            '}' => {
                chars.next();
                if !current_name.is_empty() {
                    parts.push(std::mem::take(&mut current_name));
                }
                return Ok(parts.join(" "));
            }
            ',' => {
                chars.next();
                if !current_name.is_empty() {
                    parts.push(std::mem::take(&mut current_name));
                }
            }
            _ => {
                current_name.push(c);
                chars.next();
            }
        }
    }

    if !current_name.is_empty() {
        parts.push(current_name);
    }
    Ok(parts.join(" "))
}

/// A built GraphQL query ready to POST.
#[derive(Debug, Clone)]
pub struct BuiltQuery {
    pub query: String,
    pub variables: Map<String, Value>,
}

/// Builds a GraphQL query for the given top-level field.
///
/// - `field_name`: the name as it appears in the operation (e.g., "products")
/// - `field_def`: the field's schema definition (used to look up arg types and return type)
/// - `args`: parsed user arguments (only ones matching declared args are used)
/// - `selection`: pre-built selection set string (e.g. "{ items { sku } total_count }").
///   If None, uses smart defaults from the return type.
/// - `schema`: the full schema (for type lookups)
pub fn build_query(
    field_name: &str,
    field_def: &FieldDef,
    args: &Map<String, Value>,
    selection: Option<&str>,
    schema: &GraphQLSchema,
) -> Result<BuiltQuery> {
    build_operation("query", field_name, field_def, args, selection, schema)
}

pub fn build_mutation(
    field_name: &str,
    field_def: &FieldDef,
    args: &Map<String, Value>,
    selection: Option<&str>,
    schema: &GraphQLSchema,
) -> Result<BuiltQuery> {
    build_operation("mutation", field_name, field_def, args, selection, schema)
}

fn build_operation(
    operation_kind: &str,
    field_name: &str,
    field_def: &FieldDef,
    args: &Map<String, Value>,
    selection: Option<&str>,
    schema: &GraphQLSchema,
) -> Result<BuiltQuery> {
    // Determine which user-provided args match declared args
    let mut variable_decls: Vec<String> = Vec::new();
    let mut call_args: Vec<String> = Vec::new();
    let mut variables: Map<String, Value> = Map::new();

    for arg_def in &field_def.args {
        if let Some(value) = args.get(&arg_def.name) {
            let coerced = coerce_value_for_type(value, &arg_def.type_ref, schema)
                .map_err(|e| anyhow!("invalid value for argument '{}': {}", arg_def.name, e))?;
            variable_decls.push(format!("${}: {}", arg_def.name, arg_def.type_ref.render()));
            call_args.push(format!("{}: ${}", arg_def.name, arg_def.name));
            variables.insert(arg_def.name.clone(), coerced);
        }
    }

    // Reject unknown args
    for k in args.keys() {
        if !field_def.args.iter().any(|a| &a.name == k) {
            return Err(anyhow!(
                "argument '{}' is not declared on {}",
                k,
                field_name
            ));
        }
    }

    // Build the selection set
    let selection_str = match selection {
        Some(s) => s.to_string(),
        None => {
            let return_name = field_def.type_ref.unwrap_name();
            match return_name.and_then(|n| schema.types.get(n)) {
                Some(t) if matches!(t.kind, TypeKind::Object | TypeKind::Interface) => {
                    smart_select_fields(schema, t, 0)
                }
                _ => String::new(), // scalar return — no selection
            }
        }
    };

    let var_decls_str = if variable_decls.is_empty() {
        String::new()
    } else {
        format!("({})", variable_decls.join(", "))
    };

    let call_args_str = if call_args.is_empty() {
        String::new()
    } else {
        format!("({})", call_args.join(", "))
    };

    let query = format!(
        "{op}{vars} {{ {field}{call}{sel} }}",
        op = operation_kind,
        vars = var_decls_str,
        field = field_name,
        call = call_args_str,
        sel = if selection_str.is_empty() {
            String::new()
        } else {
            format!(" {}", selection_str)
        },
    );

    Ok(BuiltQuery { query, variables })
}

fn sample_scalar_literal(type_name: &str, field_name_hint: &str) -> String {
    let hint = field_name_hint.to_ascii_lowercase();
    match type_name {
        "Boolean" => "true".to_string(),
        "Int" => "1".to_string(),
        "Float" => "1.5".to_string(),
        "String" | "ID" => {
            if hint.contains("code") {
                if hint.contains("continent") {
                    "EU".to_string()
                } else if hint.contains("language") {
                    "en".to_string()
                } else if hint.contains("currency") {
                    "USD".to_string()
                } else {
                    "US".to_string()
                }
            } else if hint.contains("search") || hint.contains("query") || hint.contains("term") {
                "milk".to_string()
            } else if hint.contains("name") {
                "example".to_string()
            } else if hint.contains("id") {
                "abc123".to_string()
            } else {
                "value".to_string()
            }
        }
        _ => "value".to_string(),
    }
}

fn rank_input_field(name: &str) -> usize {
    match name {
        "eq" => 0,
        "id" => 1,
        "code" => 2,
        "name" => 3,
        "search" => 4,
        "query" => 5,
        _ => 100,
    }
}

fn example_arg_token_for_type(
    field_name: &str,
    prefix: &str,
    type_ref: &TypeRef,
    schema: &GraphQLSchema,
) -> Option<String> {
    match type_ref.kind {
        TypeKind::NonNull => type_ref
            .of_type
            .as_ref()
            .and_then(|inner| example_arg_token_for_type(field_name, prefix, inner, schema)),
        TypeKind::List => None,
        _ => {
            let type_name = type_ref.unwrap_name()?;
            match schema.types.get(type_name) {
                Some(type_def) if matches!(type_def.kind, TypeKind::InputObject) => {
                    let field = type_def.input_fields.iter().min_by_key(|f| {
                        let required = !(matches!(f.type_ref.kind, TypeKind::NonNull)
                            && f.default_value.is_none());
                        (required, rank_input_field(&f.name), &f.name)
                    })?;
                    let next_prefix = format!("{}.{}", prefix, field.name);
                    example_arg_token_for_type(field_name, &next_prefix, &field.type_ref, schema)
                }
                Some(type_def) if matches!(type_def.kind, TypeKind::Enum) => type_def
                    .enum_values
                    .first()
                    .map(|value| format!("{}:{}", prefix, value)),
                _ => Some(format!(
                    "{}:{}",
                    prefix,
                    sample_scalar_literal(type_name, &format!("{}.{}", field_name, prefix))
                )),
            }
        }
    }
}

fn smart_select_fields_cli_inner(
    schema: &GraphQLSchema,
    type_def: &TypeDef,
    depth: usize,
) -> String {
    let mut parts: Vec<String> = Vec::new();

    for field in &type_def.fields {
        if field_has_required_arg(field) {
            continue;
        }

        let inner = unwrap_type_ref(&field.type_ref);
        let inner_name = inner.name.as_deref();
        let inner_type = inner_name.and_then(|n| schema.types.get(n));
        let (kind, nested_type) = match inner_type {
            Some(t) => (t.kind.clone(), Some(t)),
            None => (inner.kind.clone(), None),
        };

        if is_scalar_kind(&kind) {
            parts.push(field.name.clone());
        } else if matches!(kind, TypeKind::Object | TypeKind::Interface)
            && depth < MAX_SELECTION_DEPTH
            && let Some(t) = nested_type
        {
            let nested = smart_select_fields_cli_inner(schema, t, depth + 1);
            if !nested.is_empty() {
                parts.push(format!("{}{{{}}}", field.name, nested));
            }
        }
    }

    parts.join(",")
}

pub fn example_command_for_field(
    command: &str,
    field_name: &str,
    field_def: &FieldDef,
    schema: &GraphQLSchema,
) -> String {
    let mut arg_tokens: Vec<String> = field_def
        .args
        .iter()
        .filter(|arg| matches!(arg.type_ref.kind, TypeKind::NonNull) && arg.default_value.is_none())
        .filter_map(|arg| example_arg_token_for_type(field_name, &arg.name, &arg.type_ref, schema))
        .collect();

    if arg_tokens.is_empty()
        && let Some(arg) = field_def
            .args
            .iter()
            .find(|arg| !input_field_has_required_arg(&arg.type_ref, schema))
            .or_else(|| field_def.args.first())
        && let Some(token) =
            example_arg_token_for_type(field_name, &arg.name, &arg.type_ref, schema)
    {
        arg_tokens.push(token);
    }

    let selection = field_def
        .type_ref
        .unwrap_name()
        .and_then(|n| schema.types.get(n))
        .filter(|t| matches!(t.kind, TypeKind::Object | TypeKind::Interface))
        .map(|t| smart_select_fields_cli_inner(schema, t, 0))
        .filter(|s| !s.is_empty());

    let mut parts = vec![command.to_string()];
    parts.extend(arg_tokens);
    if let Some(fields) = selection {
        parts.push("--fields".to_string());
        parts.push(fields);
    }

    parts.join(" ")
}

/// Executes a BuiltQuery against the configured endpoint and returns the parsed response body.
pub fn execute_query(
    url: &str,
    headers: &HashMap<String, String>,
    auth: Option<&AuthConfig>,
    built: &BuiltQuery,
) -> Result<(u16, Value)> {
    let payload = serde_json::json!({
        "query": built.query,
        "variables": built.variables,
    });
    let response = post_graphql_json(url, headers, auth, &payload)?;
    let status = response.status;
    let body_text = response.body_text;
    let body: Value =
        serde_json::from_str(&body_text).unwrap_or_else(|_| Value::String(body_text.clone()));
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_ref_render_scalar() {
        let t = TypeRef {
            kind: TypeKind::Scalar,
            name: Some("String".to_string()),
            of_type: None,
        };
        assert_eq!(t.render(), "String");
    }

    #[test]
    fn type_ref_render_non_null_list_non_null() {
        // [Product!]!
        let t = TypeRef {
            kind: TypeKind::NonNull,
            name: None,
            of_type: Some(Box::new(TypeRef {
                kind: TypeKind::List,
                name: None,
                of_type: Some(Box::new(TypeRef {
                    kind: TypeKind::NonNull,
                    name: None,
                    of_type: Some(Box::new(TypeRef {
                        kind: TypeKind::Object,
                        name: Some("Product".to_string()),
                        of_type: None,
                    })),
                })),
            })),
        };
        assert_eq!(t.render(), "[Product!]!");
    }

    #[test]
    fn type_ref_unwrap_name_through_wrappers() {
        let t = TypeRef {
            kind: TypeKind::NonNull,
            name: None,
            of_type: Some(Box::new(TypeRef {
                kind: TypeKind::List,
                name: None,
                of_type: Some(Box::new(TypeRef {
                    kind: TypeKind::Object,
                    name: Some("Product".to_string()),
                    of_type: None,
                })),
            })),
        };
        assert_eq!(t.unwrap_name(), Some("Product"));
    }

    #[test]
    fn parse_minimal_introspection() {
        let json = r#"{
          "data": {
            "__schema": {
              "queryType": { "name": "Query" },
              "mutationType": null,
              "types": [
                {
                  "kind": "OBJECT",
                  "name": "Query",
                  "fields": [
                    {
                      "name": "hello",
                      "description": null,
                      "args": [],
                      "type": { "kind": "SCALAR", "name": "String", "ofType": null }
                    }
                  ],
                  "inputFields": null,
                  "enumValues": null
                },
                {
                  "kind": "SCALAR",
                  "name": "String",
                  "fields": null,
                  "inputFields": null,
                  "enumValues": null
                }
              ]
            }
          }
        }"#;

        let schema = parse_introspection(json).unwrap();
        assert_eq!(schema.query_type, "Query");
        assert_eq!(schema.mutation_type, None);
        let query_type = schema.types.get("Query").unwrap();
        assert_eq!(query_type.fields.len(), 1);
        assert_eq!(query_type.fields[0].name, "hello");
        assert_eq!(query_type.fields[0].type_ref.render(), "String");
    }

    #[test]
    fn detects_akamai_style_waf_block_pages() {
        let body = r#"<html><body><h1>Access Denied</h1>Reference #18.14793517.1775919278.92db5eec https://errors.edgesuite.net/</body></html>"#;
        assert!(looks_like_waf_block(403, Some("text/html"), body));
    }

    #[test]
    fn build_introspection_query_limits_of_type_depth() {
        let shallow = build_introspection_query(2);
        assert!(shallow.contains("ofType { kind name ofType { kind name"));
        assert!(
            !shallow.contains("ofType { kind name ofType { kind name ofType { kind name ofType")
        );
    }

    #[test]
    fn detects_query_depth_limit_errors() {
        let body = r#"{"errors":[{"message":"Query depth limit exceeded.","extensions":{"code":"GCDN_QUERY_DEPTH_LIMIT","httpStatusCode":413}}]}"#;
        assert!(is_query_depth_limit_error(413, body));
        assert!(!is_query_depth_limit_error(400, body));
    }

    #[test]
    fn remembers_waf_fallback_by_url() {
        let url = "https://example.com/graphql";
        assert!(!should_use_waf_fallback(url));
        remember_waf_fallback(url);
        assert!(should_use_waf_fallback(url));
    }

    #[test]
    fn does_not_treat_json_403_as_waf_block() {
        let body = r#"{"errors":[{"message":"forbidden"}]}"#;
        assert!(!looks_like_waf_block(403, Some("application/json"), body));
    }

    fn schema_with_products() -> GraphQLSchema {
        // Build a small fake schema:
        // Query.products(search: String): Products
        // Products { items: [Product], total_count: Int }
        // Product { sku: String, item_title: String }
        let mut types = HashMap::new();

        types.insert(
            "Query".to_string(),
            TypeDef {
                name: "Query".to_string(),
                kind: TypeKind::Object,
                fields: vec![FieldDef {
                    name: "products".to_string(),
                    description: None,
                    args: vec![ArgDef {
                        name: "search".to_string(),
                        type_ref: TypeRef {
                            kind: TypeKind::Scalar,
                            name: Some("String".to_string()),
                            of_type: None,
                        },
                        default_value: None,
                    }],
                    type_ref: TypeRef {
                        kind: TypeKind::Object,
                        name: Some("Products".to_string()),
                        of_type: None,
                    },
                }],
                input_fields: vec![],
                enum_values: vec![],
            },
        );

        types.insert(
            "Products".to_string(),
            TypeDef {
                name: "Products".to_string(),
                kind: TypeKind::Object,
                fields: vec![
                    FieldDef {
                        name: "items".to_string(),
                        description: None,
                        args: vec![],
                        type_ref: TypeRef {
                            kind: TypeKind::List,
                            name: None,
                            of_type: Some(Box::new(TypeRef {
                                kind: TypeKind::Object,
                                name: Some("Product".to_string()),
                                of_type: None,
                            })),
                        },
                    },
                    FieldDef {
                        name: "total_count".to_string(),
                        description: None,
                        args: vec![],
                        type_ref: TypeRef {
                            kind: TypeKind::Scalar,
                            name: Some("Int".to_string()),
                            of_type: None,
                        },
                    },
                ],
                input_fields: vec![],
                enum_values: vec![],
            },
        );

        types.insert(
            "Product".to_string(),
            TypeDef {
                name: "Product".to_string(),
                kind: TypeKind::Object,
                fields: vec![
                    FieldDef {
                        name: "sku".to_string(),
                        description: None,
                        args: vec![],
                        type_ref: TypeRef {
                            kind: TypeKind::Scalar,
                            name: Some("String".to_string()),
                            of_type: None,
                        },
                    },
                    FieldDef {
                        name: "item_title".to_string(),
                        description: None,
                        args: vec![],
                        type_ref: TypeRef {
                            kind: TypeKind::Scalar,
                            name: Some("String".to_string()),
                            of_type: None,
                        },
                    },
                ],
                input_fields: vec![],
                enum_values: vec![],
            },
        );

        GraphQLSchema {
            query_type: "Query".to_string(),
            mutation_type: None,
            types,
        }
    }

    fn schema_with_countries_filter() -> GraphQLSchema {
        let mut types = HashMap::new();

        types.insert(
            "Query".to_string(),
            TypeDef {
                name: "Query".to_string(),
                kind: TypeKind::Object,
                fields: vec![
                    FieldDef {
                        name: "countries".to_string(),
                        description: None,
                        args: vec![ArgDef {
                            name: "filter".to_string(),
                            type_ref: TypeRef {
                                kind: TypeKind::InputObject,
                                name: Some("CountryFilterInput".to_string()),
                                of_type: None,
                            },
                            default_value: None,
                        }],
                        type_ref: TypeRef {
                            kind: TypeKind::List,
                            name: None,
                            of_type: Some(Box::new(TypeRef {
                                kind: TypeKind::Object,
                                name: Some("Country".to_string()),
                                of_type: None,
                            })),
                        },
                    },
                    FieldDef {
                        name: "continents".to_string(),
                        description: None,
                        args: vec![ArgDef {
                            name: "filter".to_string(),
                            type_ref: TypeRef {
                                kind: TypeKind::InputObject,
                                name: Some("ContinentFilterInput".to_string()),
                                of_type: None,
                            },
                            default_value: None,
                        }],
                        type_ref: TypeRef {
                            kind: TypeKind::List,
                            name: None,
                            of_type: Some(Box::new(TypeRef {
                                kind: TypeKind::Object,
                                name: Some("Continent".to_string()),
                                of_type: None,
                            })),
                        },
                    },
                ],
                input_fields: vec![],
                enum_values: vec![],
            },
        );

        types.insert(
            "Country".to_string(),
            TypeDef {
                name: "Country".to_string(),
                kind: TypeKind::Object,
                fields: vec![
                    FieldDef {
                        name: "code".to_string(),
                        description: None,
                        args: vec![],
                        type_ref: TypeRef {
                            kind: TypeKind::Scalar,
                            name: Some("String".to_string()),
                            of_type: None,
                        },
                    },
                    FieldDef {
                        name: "name".to_string(),
                        description: None,
                        args: vec![],
                        type_ref: TypeRef {
                            kind: TypeKind::Scalar,
                            name: Some("String".to_string()),
                            of_type: None,
                        },
                    },
                    FieldDef {
                        name: "capital".to_string(),
                        description: None,
                        args: vec![],
                        type_ref: TypeRef {
                            kind: TypeKind::Scalar,
                            name: Some("String".to_string()),
                            of_type: None,
                        },
                    },
                ],
                input_fields: vec![],
                enum_values: vec![],
            },
        );

        types.insert(
            "CountryFilterInput".to_string(),
            TypeDef {
                name: "CountryFilterInput".to_string(),
                kind: TypeKind::InputObject,
                fields: vec![],
                input_fields: vec![
                    InputFieldDef {
                        name: "code".to_string(),
                        type_ref: TypeRef {
                            kind: TypeKind::InputObject,
                            name: Some("StringQueryOperatorInput".to_string()),
                            of_type: None,
                        },
                        default_value: None,
                    },
                    InputFieldDef {
                        name: "name".to_string(),
                        type_ref: TypeRef {
                            kind: TypeKind::InputObject,
                            name: Some("StringQueryOperatorInput".to_string()),
                            of_type: None,
                        },
                        default_value: None,
                    },
                ],
                enum_values: vec![],
            },
        );

        types.insert(
            "Continent".to_string(),
            TypeDef {
                name: "Continent".to_string(),
                kind: TypeKind::Object,
                fields: vec![
                    FieldDef {
                        name: "code".to_string(),
                        description: None,
                        args: vec![],
                        type_ref: TypeRef {
                            kind: TypeKind::Scalar,
                            name: Some("String".to_string()),
                            of_type: None,
                        },
                    },
                    FieldDef {
                        name: "name".to_string(),
                        description: None,
                        args: vec![],
                        type_ref: TypeRef {
                            kind: TypeKind::Scalar,
                            name: Some("String".to_string()),
                            of_type: None,
                        },
                    },
                ],
                input_fields: vec![],
                enum_values: vec![],
            },
        );

        types.insert(
            "ContinentFilterInput".to_string(),
            TypeDef {
                name: "ContinentFilterInput".to_string(),
                kind: TypeKind::InputObject,
                fields: vec![],
                input_fields: vec![
                    InputFieldDef {
                        name: "code".to_string(),
                        type_ref: TypeRef {
                            kind: TypeKind::InputObject,
                            name: Some("StringQueryOperatorInput".to_string()),
                            of_type: None,
                        },
                        default_value: None,
                    },
                    InputFieldDef {
                        name: "name".to_string(),
                        type_ref: TypeRef {
                            kind: TypeKind::InputObject,
                            name: Some("StringQueryOperatorInput".to_string()),
                            of_type: None,
                        },
                        default_value: None,
                    },
                ],
                enum_values: vec![],
            },
        );

        types.insert(
            "StringQueryOperatorInput".to_string(),
            TypeDef {
                name: "StringQueryOperatorInput".to_string(),
                kind: TypeKind::InputObject,
                fields: vec![],
                input_fields: vec![
                    InputFieldDef {
                        name: "eq".to_string(),
                        type_ref: TypeRef {
                            kind: TypeKind::Scalar,
                            name: Some("String".to_string()),
                            of_type: None,
                        },
                        default_value: None,
                    },
                    InputFieldDef {
                        name: "regex".to_string(),
                        type_ref: TypeRef {
                            kind: TypeKind::Scalar,
                            name: Some("String".to_string()),
                            of_type: None,
                        },
                        default_value: None,
                    },
                ],
                enum_values: vec![],
            },
        );

        GraphQLSchema {
            query_type: "Query".to_string(),
            mutation_type: None,
            types,
        }
    }

    #[test]
    fn resolve_root() {
        let schema = schema_with_products();
        let nav = resolve_path(&schema, "/").unwrap();
        match nav {
            Navigation::Root => {}
            _ => panic!("expected Root"),
        }
    }

    #[test]
    fn resolve_query_field() {
        let schema = schema_with_products();
        let nav = resolve_path(&schema, "/products").unwrap();
        match nav {
            Navigation::Field {
                type_name,
                field_name,
                ..
            } => {
                assert_eq!(type_name, "Query");
                assert_eq!(field_name, "products");
            }
            _ => panic!("expected Field"),
        }
    }

    #[test]
    fn resolve_nested_field() {
        let schema = schema_with_products();
        let nav = resolve_path(&schema, "/products/items").unwrap();
        match nav {
            Navigation::Field {
                type_name,
                field_name,
                ..
            } => {
                assert_eq!(type_name, "Products");
                assert_eq!(field_name, "items");
            }
            _ => panic!("expected Field"),
        }
    }

    #[test]
    fn resolve_invalid_path_returns_none() {
        let schema = schema_with_products();
        assert!(resolve_path(&schema, "/nonexistent").is_none());
    }

    #[test]
    fn parse_simple_string_arg() {
        let args = parse_args(&["search:milk"]).unwrap();
        assert_eq!(args.get("search").unwrap(), &serde_json::json!("milk"));
    }

    #[test]
    fn parse_quoted_string_arg() {
        let args = parse_args(&["search:\"hello world\""]).unwrap();
        assert_eq!(
            args.get("search").unwrap(),
            &serde_json::json!("hello world")
        );
    }

    #[test]
    fn parse_int_arg() {
        let args = parse_args(&["pageSize:3"]).unwrap();
        assert_eq!(args.get("pageSize").unwrap(), &serde_json::json!(3));
    }

    #[test]
    fn parse_bool_arg() {
        let args = parse_args(&["enabled:true"]).unwrap();
        assert_eq!(args.get("enabled").unwrap(), &serde_json::json!(true));
    }

    #[test]
    fn parse_nested_arg() {
        let args = parse_args(&["filter.store_code.eq:TJ"]).unwrap();
        assert_eq!(
            args.get("filter").unwrap(),
            &serde_json::json!({"store_code": {"eq": "TJ"}})
        );
    }

    #[test]
    fn parse_multiple_nested_into_same_object() {
        let args = parse_args(&["filter.store_code.eq:TJ", "filter.published.eq:1"]).unwrap();
        assert_eq!(
            args.get("filter").unwrap(),
            &serde_json::json!({
                "store_code": {"eq": "TJ"},
                "published": {"eq": 1}
            })
        );
    }

    #[test]
    fn smart_select_scalars_only() {
        let schema = schema_with_products();
        // From inside `products`, the return type is Products.
        // Smart select should pick: items{sku item_title} total_count
        let products_type = schema.types.get("Products").unwrap();
        let selection = smart_select_fields(&schema, products_type, 0);
        // Expect both items and total_count
        assert!(selection.contains("total_count"));
        assert!(selection.contains("items"));
        assert!(selection.contains("sku"));
        assert!(selection.contains("item_title"));
    }

    #[test]
    fn parse_simple_field_list() {
        let s = parse_field_selection("sku,item_title").unwrap();
        assert_eq!(s, "{ sku item_title }");
    }

    #[test]
    fn parse_nested_field_list() {
        let s = parse_field_selection("items{sku,item_title},total_count").unwrap();
        assert_eq!(s, "{ items { sku item_title } total_count }");
    }

    #[test]
    fn parse_deeply_nested() {
        let s = parse_field_selection("a{b{c,d}}").unwrap();
        assert_eq!(s, "{ a { b { c d } } }");
    }

    #[test]
    fn build_query_with_args_and_smart_fields() {
        let schema = schema_with_products();
        let products_field = &schema.types.get("Query").unwrap().fields[0];
        let mut args = Map::new();
        args.insert("search".to_string(), Value::String("milk".to_string()));

        let built = build_query("products", products_field, &args, None, &schema).unwrap();

        // Should declare $search:String, call products(search:$search) and select scalars
        assert!(built.query.contains("$search: String"));
        assert!(built.query.contains("products(search: $search)"));
        assert!(built.query.contains("total_count"));
        assert!(built.query.contains("items"));
        assert_eq!(
            built.variables.get("search").unwrap(),
            &Value::String("milk".to_string())
        );
    }

    #[test]
    fn build_query_coerces_quoted_int_to_int() {
        let mut schema = schema_with_products();
        schema.types.get_mut("Query").unwrap().fields[0]
            .args
            .push(ArgDef {
                name: "pageSize".to_string(),
                type_ref: TypeRef {
                    kind: TypeKind::Scalar,
                    name: Some("Int".to_string()),
                    of_type: None,
                },
                default_value: None,
            });

        let products_field = &schema.types.get("Query").unwrap().fields[0];
        let mut args = Map::new();
        args.insert("pageSize".to_string(), Value::String("3".to_string()));

        let built = build_query("products", products_field, &args, None, &schema).unwrap();
        assert_eq!(
            built.variables.get("pageSize").unwrap(),
            &serde_json::json!(3)
        );
    }

    #[test]
    fn build_query_rejects_unknown_nested_input_field() {
        let schema = schema_with_countries_filter();
        let countries_field = &schema.types.get("Query").unwrap().fields[0];
        let mut args = Map::new();
        args.insert(
            "filter".to_string(),
            serde_json::json!({"bogus": {"eq": "US"}}),
        );

        let err = build_query("countries", countries_field, &args, None, &schema).unwrap_err();
        assert!(err.to_string().contains("CountryFilterInput"));
    }

    #[test]
    fn example_command_uses_nested_input_shape() {
        let schema = schema_with_countries_filter();
        let countries_field = &schema.types.get("Query").unwrap().fields[0];

        let example = example_command_for_field("query", "countries", countries_field, &schema);
        assert_eq!(
            example,
            "query filter.code.eq:US --fields code,name,capital"
        );
    }

    #[test]
    fn example_command_uses_field_context_for_continent_codes() {
        let schema = schema_with_countries_filter();
        let continents_field = &schema.types.get("Query").unwrap().fields[1];

        let example = example_command_for_field("query", "continents", continents_field, &schema);
        assert_eq!(example, "query filter.code.eq:EU --fields code,name");
    }

    #[test]
    fn smart_select_skips_args_required_fields() {
        // If a field has required args, skip it.
        let mut schema = schema_with_products();
        schema
            .types
            .get_mut("Products")
            .unwrap()
            .fields
            .push(FieldDef {
                name: "needs_arg".to_string(),
                description: None,
                args: vec![ArgDef {
                    name: "id".to_string(),
                    type_ref: TypeRef {
                        kind: TypeKind::NonNull,
                        name: None,
                        of_type: Some(Box::new(TypeRef {
                            kind: TypeKind::Scalar,
                            name: Some("ID".to_string()),
                            of_type: None,
                        })),
                    },
                    default_value: None,
                }],
                type_ref: TypeRef {
                    kind: TypeKind::Scalar,
                    name: Some("String".to_string()),
                    of_type: None,
                },
            });
        let products_type = schema.types.get("Products").unwrap();
        let selection = smart_select_fields(&schema, products_type, 0);
        assert!(!selection.contains("needs_arg"));
    }
}
