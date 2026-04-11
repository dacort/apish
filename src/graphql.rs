use crate::config::{AuthConfig, KeyLocation};
use anyhow::{Result, anyhow};
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::time::Duration;

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

pub const INTROSPECTION_QUERY: &str = r#"{
  __schema {
    queryType { name }
    mutationType { name }
    types {
      kind
      name
      fields(includeDeprecated: true) {
        name
        description
        args {
          name
          type { ...TypeRef }
          defaultValue
        }
        type { ...TypeRef }
      }
      inputFields {
        name
        type { ...TypeRef }
        defaultValue
      }
      enumValues(includeDeprecated: true) { name }
    }
  }
}

fragment TypeRef on __Type {
  kind
  name
  ofType {
    kind
    name
    ofType {
      kind
      name
      ofType {
        kind
        name
        ofType {
          kind
          name
          ofType {
            kind
            name
            ofType {
              kind
              name
              ofType {
                kind
                name
              }
            }
          }
        }
      }
    }
  }
}"#;

/// Fetches the schema from a GraphQL endpoint by POSTing the introspection query.
pub fn fetch_schema(
    url: &str,
    headers: &HashMap<String, String>,
    auth: Option<&AuthConfig>,
) -> Result<GraphQLSchema> {
    let client = Client::builder().timeout(Duration::from_secs(30)).build()?;

    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .body(format!(
            "{{\"query\": {}}}",
            serde_json::to_string(INTROSPECTION_QUERY)?
        ));

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

    let response = req.send()?;
    if !response.status().is_success() {
        return Err(anyhow!(
            "introspection failed with status {}",
            response.status()
        ));
    }

    let body = response.text()?;
    parse_introspection(&body)
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
    let client = match Client::builder().timeout(Duration::from_secs(10)).build() {
        Ok(c) => c,
        Err(_) => return false,
    };
    let body = format!("{{\"query\": \"{{ __schema {{ queryType {{ name }} }} }}\"}}");
    let resp = match client
        .post(url)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
    {
        Ok(r) => r,
        Err(_) => return false,
    };
    if !resp.status().is_success() {
        return false;
    }
    let text = match resp.text() {
        Ok(t) => t,
        Err(_) => return false,
    };
    // Look for the structural marker
    text.contains("\"__schema\"") && text.contains("\"queryType\"")
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
            variable_decls.push(format!("${}: {}", arg_def.name, arg_def.type_ref.render()));
            call_args.push(format!("{}: ${}", arg_def.name, arg_def.name));
            variables.insert(arg_def.name.clone(), value.clone());
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

/// Executes a BuiltQuery against the configured endpoint and returns the parsed response body.
pub fn execute_query(
    url: &str,
    headers: &HashMap<String, String>,
    auth: Option<&AuthConfig>,
    built: &BuiltQuery,
) -> Result<(u16, Value)> {
    let client = Client::builder().timeout(Duration::from_secs(30)).build()?;

    let payload = serde_json::json!({
        "query": built.query,
        "variables": built.variables,
    });

    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&payload)?);

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

    let response = req.send()?;
    let status = response.status().as_u16();
    let body_text = response.text()?;
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
