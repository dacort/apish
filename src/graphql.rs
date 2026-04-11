use crate::config::{AuthConfig, KeyLocation};
use anyhow::{Result, anyhow};
use reqwest::blocking::Client;
use serde::Deserialize;
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
                let inner = self.of_type.as_ref().map(|t| t.render()).unwrap_or_default();
                format!("{}!", inner)
            }
            TypeKind::List => {
                let inner = self.of_type.as_ref().map(|t| t.render()).unwrap_or_default();
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
            .map(|fs| fs.iter().map(convert_input_field).collect::<Result<Vec<_>>>())
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
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

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
            AuthConfig::ApiKey { name, value, location } => match location {
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
            Navigation::Field { type_name, field_name, .. } => {
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
            Navigation::Field { type_name, field_name, .. } => {
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
}
