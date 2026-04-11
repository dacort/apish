use anyhow::{Result, anyhow};
use serde::Deserialize;
use std::collections::HashMap;

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
}
