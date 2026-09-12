//! The settings schema: the input parameters reactivepay sends in the
//! `settings` bucket of every request.
//!
//! Credentials arrive per-request and are **never stored** by this service —
//! the schema declares their shape so the editor can render a form and so the
//! platform registration manifest can be generated, nothing more.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsSchema {
    #[serde(default)]
    pub fields: Vec<FieldDef>,
}

impl SettingsSchema {
    pub fn get(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.name == name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.fields.iter().map(|f| f.name.as_str())
    }

    /// Field names for the platform's `params_fields.settings` manifest.
    pub fn manifest(&self) -> Vec<String> {
        self.fields.iter().map(|f| f.name.clone()).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldDef {
    /// Key within the inbound `settings` object.
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub ty: FieldType,
    #[serde(default)]
    pub required: bool,
    /// Masked in the editor and redacted from interaction logs.
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub help: Option<String>,
    #[serde(default)]
    pub default: Option<Value>,
}

impl FieldDef {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            label: None,
            ty: FieldType::Text,
            required: false,
            secret: false,
            help: None,
            default: None,
        }
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn secret(mut self) -> Self {
        self.secret = true;
        self
    }

    pub fn ty(mut self, ty: FieldType) -> Self {
        self.ty = ty;
        self
    }

    pub fn display(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FieldType {
    #[default]
    Text,
    Number,
    Bool,
    Select {
        options: Vec<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let schema = SettingsSchema {
            fields: vec![
                FieldDef::new("client_id").required(),
                FieldDef::new("client_secret").required().secret(),
                FieldDef::new("sandbox").ty(FieldType::Bool),
            ],
        };
        let json = serde_json::to_string(&schema).unwrap();
        assert_eq!(
            serde_json::from_str::<SettingsSchema>(&json).unwrap(),
            schema
        );
        assert_eq!(schema.manifest(), ["client_id", "client_secret", "sandbox"]);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let err = serde_json::from_str::<FieldDef>(r#"{"name":"a","secrt":true}"#).unwrap_err();
        assert!(err.to_string().contains("unknown field"), "{err}");
    }
}
