use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Metadata for a Minecraft instance (server or client).
///
/// Inspired by SJMCL's instance model which treats instances as first-class
/// citizens with rich metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstanceMetadata {
    /// Human-readable name (may differ from instance ID).
    pub name: String,
    /// Optional description.
    pub description: String,
    /// Path to an icon image (optional).
    pub icon_path: Option<PathBuf>,
    /// User-defined tags for categorization.
    pub tags: Vec<String>,
    /// Whether this instance is starred/favorited.
    pub starred: bool,
    /// When the instance was created.
    pub created_at: DateTime<Utc>,
    /// When the instance was last played/started.
    pub last_played: Option<DateTime<Utc>>,
}

impl Default for InstanceMetadata {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            icon_path: None,
            tags: Vec::new(),
            starred: false,
            created_at: Utc::now(),
            last_played: None,
        }
    }
}

impl InstanceMetadata {
    /// Create metadata with just a name.
    #[must_use]
    pub fn with_name(name: String) -> Self {
        Self {
            name,
            ..Default::default()
        }
    }

    /// Add a tag.
    pub fn add_tag(&mut self, tag: String) {
        if !self.tags.contains(&tag) {
            self.tags.push(tag);
        }
    }

    /// Remove a tag.
    pub fn remove_tag(&mut self, tag: &str) {
        self.tags.retain(|t| t != tag);
    }

    /// Update `last_played` to now.
    pub fn mark_played(&mut self) {
        self.last_played = Some(Utc::now());
    }

    /// Check if the instance has a given tag.
    #[must_use]
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.contains(&tag.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_default() {
        let meta = InstanceMetadata::default();
        assert!(meta.name.is_empty());
        assert!(meta.description.is_empty());
        assert!(meta.icon_path.is_none());
        assert!(meta.tags.is_empty());
        assert!(!meta.starred);
        assert!(meta.last_played.is_none());
    }

    #[test]
    fn test_metadata_with_name() {
        let meta = InstanceMetadata::with_name("Survival Server".to_string());
        assert_eq!(meta.name, "Survival Server");
    }

    #[test]
    fn test_metadata_add_remove_tag() {
        let mut meta = InstanceMetadata::default();
        meta.add_tag("pvp".to_string());
        meta.add_tag("survival".to_string());
        assert!(meta.has_tag("pvp"));
        assert!(meta.has_tag("survival"));

        meta.remove_tag("pvp");
        assert!(!meta.has_tag("pvp"));
        assert!(meta.has_tag("survival"));
    }

    #[test]
    fn test_metadata_duplicate_tag() {
        let mut meta = InstanceMetadata::default();
        meta.add_tag("modded".to_string());
        meta.add_tag("modded".to_string());
        assert_eq!(meta.tags.len(), 1);
    }

    #[test]
    fn test_metadata_mark_played() {
        let mut meta = InstanceMetadata::default();
        assert!(meta.last_played.is_none());
        meta.mark_played();
        assert!(meta.last_played.is_some());
    }

    #[test]
    fn test_metadata_serde() {
        let meta = InstanceMetadata {
            name: "Test".into(),
            description: "A test instance".into(),
            icon_path: Some(PathBuf::from("/icons/test.png")),
            tags: vec!["survival".into(), "1.20".into()],
            starred: true,
            created_at: Utc::now(),
            last_played: Some(Utc::now()),
        };

        let json = serde_json::to_string(&meta).unwrap();
        let decoded: InstanceMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.name, "Test");
        assert_eq!(decoded.tags, vec!["survival", "1.20"]);
        assert!(decoded.starred);
    }
}
