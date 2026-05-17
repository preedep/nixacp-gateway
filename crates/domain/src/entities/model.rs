use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::sync::Arc;

/// Cheap-to-clone model identifier backed by `Arc<str>`.
/// Uses String for serde to avoid needing the `rc` serde feature.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelId(pub Arc<str>);

impl ModelId {
    pub fn new(s: impl AsRef<str>) -> Self {
        Self(Arc::from(s.as_ref()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for ModelId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ModelId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(ModelId::new(s))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDescriptor {
    pub id: ModelId,
    pub owned_by: String,
    pub description: Option<String>,
    pub max_tokens: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_id_display() {
        let id = ModelId::new("qwen25-chat-local:latest");
        assert_eq!(id.to_string(), "qwen25-chat-local:latest");
    }

    #[test]
    fn model_id_serde_round_trip() {
        let id = ModelId::new("test-model");
        let json = serde_json::to_string(&id).unwrap();
        let back: ModelId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }
}
