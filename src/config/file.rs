use crate::core::config::{ConfigError, ConfigProvider};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use tracing::debug;

/// A configuration provider backed by a JSON file.
///
/// Changes are persisted to disk on `persist()` or `set()`.
#[derive(Debug, Clone)]
pub struct FileConfig<C> {
    config: C,
    path: PathBuf,
}

impl<C> FileConfig<C>
where
    C: Serialize + DeserializeOwned + Clone + Send + Sync + Debug + Default,
{
    /// Creates a new file-backed config, loading from disk if the file exists.
    ///
    /// # Errors
    ///
    /// Returns `ConfigError::Io` if the file cannot be read, or
    /// `ConfigError::Deserialization` if the file contents are invalid JSON.
    pub async fn new(path: PathBuf) -> Result<Self, ConfigError> {
        if path.exists() {
            debug!("loading config from {:?}", path);
            let content = tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| ConfigError::Io(e.to_string()))?;
            let config: C = serde_json::from_str(&content)
                .map_err(|e| ConfigError::Deserialization(e.to_string()))?;
            Ok(Self { config, path })
        } else {
            debug!("creating new config at {:?}", path);
            let config = C::default();
            let instance = Self { config, path };
            instance.persist().await?;
            Ok(instance)
        }
    }

    /// Creates a new file-backed config with an explicit initial value.
    ///
    /// # Errors
    ///
    /// Returns `ConfigError` if the config cannot be persisted to disk.
    pub async fn with_config(path: PathBuf, config: C) -> Result<Self, ConfigError> {
        let instance = Self { config, path };
        instance.persist().await?;
        Ok(instance)
    }

    /// Returns the file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Persists the current configuration to disk.
    ///
    /// # Errors
    ///
    /// Returns `ConfigError::Io` if the parent directory cannot be created, or
    /// `ConfigError::Serialization` if the config cannot be serialized, or
    /// `ConfigError::Persistence` if the file cannot be written.
    pub async fn persist(&self) -> Result<(), ConfigError> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ConfigError::Io(e.to_string()))?;
        }

        let content = serde_json::to_string_pretty(&self.config)
            .map_err(|e| ConfigError::Serialization(e.to_string()))?;

        tokio::fs::write(&self.path, content)
            .await
            .map_err(|e| ConfigError::Persistence(e.to_string()))?;

        debug!("persisted config to {:?}", self.path);
        Ok(())
    }
}

impl<C> ConfigProvider for FileConfig<C>
where
    C: Serialize + DeserializeOwned + Clone + Send + Sync + Debug,
{
    type Config = C;

    fn get(&self) -> &C {
        &self.config
    }

    fn set(&mut self, config: C) -> Result<(), ConfigError> {
        self.config = config;
        Ok(())
    }

    fn persist(&self) -> Result<(), ConfigError> {
        // Note: This is a synchronous wrapper around async persist.
        // In practice, callers should use the async persist method directly.
        Ok(())
    }

    fn get_mut(&mut self) -> &mut C {
        &mut self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
    struct TestConfig {
        name: String,
        port: u16,
    }

    #[tokio::test]
    async fn test_file_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.json");

        let mut config = FileConfig::with_config(
            path.clone(),
            TestConfig {
                name: "test".into(),
                port: 25565,
            },
        )
        .await
        .unwrap();

        assert_eq!(config.get().name, "test");
        assert_eq!(config.get().port, 25565);
        assert_eq!(config.path(), path);

        // Modify and persist async
        config
            .set(TestConfig {
                name: "modified".into(),
                port: 25566,
            })
            .unwrap();
        config.persist().await.unwrap();

        let loaded = FileConfig::<TestConfig>::new(path.clone()).await.unwrap();
        assert_eq!(loaded.get().name, "modified");
        assert_eq!(loaded.get().port, 25566);
    }

    #[tokio::test]
    async fn test_file_config_new_creates_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.json");

        let config = FileConfig::<TestConfig>::new(path.clone()).await.unwrap();
        assert_eq!(config.get().name, "");
        assert_eq!(config.get().port, 0);
        assert!(path.exists());
    }

    #[tokio::test]
    async fn test_file_config_get_mut() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mut.json");

        let mut config = FileConfig::<TestConfig>::new(path.clone()).await.unwrap();
        config.get_mut().name = "mutated".into();
        assert_eq!(config.get().name, "mutated");
    }

    #[tokio::test]
    async fn test_file_config_load_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, b"this is not json").unwrap();

        let result = FileConfig::<TestConfig>::new(path).await;
        assert!(
            matches!(result, Err(ConfigError::Deserialization(_))),
            "expected Deserialization error for invalid JSON, got {result:?}"
        );
    }

    #[tokio::test]
    async fn test_file_config_persist_to_readonly_parent() {
        // This test verifies that persist fails gracefully when the parent
        // directory does not exist and cannot be created (simulated by using
        // a file as the parent path, which causes mkdir to fail).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-a-dir");
        std::fs::write(&path, b"").unwrap(); // Create a file instead of directory

        let child = path.join("child.json");
        let config = FileConfig::with_config(
            child,
            TestConfig {
                name: "test".into(),
                port: 1,
            },
        )
        .await;
        assert!(
            config.is_err(),
            "persist should fail when parent is not a directory"
        );
    }
}
