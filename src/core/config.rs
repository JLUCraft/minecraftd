use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt::Debug;
use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    #[error("configuration not found")]
    NotFound,
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("deserialization error: {0}")]
    Deserialization(String),
    #[error("persistence error: {0}")]
    Persistence(String),
    #[error("validation error: {0}")]
    Validation(String),
    #[error("io error: {0}")]
    Io(String),
}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Trait for configuration providers.
pub trait ConfigProvider: Send + Sync + Debug {
    type Config: Serialize + DeserializeOwned + Clone + Send + Sync + Debug;
    fn get(&self) -> &Self::Config;
    /// Sets the configuration value.
    ///
    /// # Errors
    ///
    /// Returns `ConfigError` if validation fails.
    fn set(&mut self, config: Self::Config) -> Result<(), ConfigError>;
    /// Persists the configuration.
    ///
    /// # Errors
    ///
    /// Returns `ConfigError` if persistence fails.
    fn persist(&self) -> Result<(), ConfigError>;
    fn get_mut(&mut self) -> &mut Self::Config;
}

/// A simple in-memory configuration provider.
#[derive(Debug, Clone)]
pub struct MemoryConfig<C> {
    config: C,
}

impl<C> MemoryConfig<C> {
    /// Creates a new `MemoryConfig`.
    pub const fn new(config: C) -> Self {
        Self { config }
    }
}

impl<C> ConfigProvider for MemoryConfig<C>
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

    #[test]
    fn test_memory_config() {
        let cfg = TestConfig {
            name: "test".into(),
            port: 25565,
        };
        let mut provider = MemoryConfig::new(cfg.clone());
        assert_eq!(provider.get().name, "test");

        let mut new_cfg = cfg;
        new_cfg.port = 25566;
        provider.set(new_cfg).unwrap();
        assert_eq!(provider.get().port, 25566);
    }

    #[test]
    fn test_memory_config_get_mut() {
        let cfg = TestConfig {
            name: "test".into(),
            port: 25565,
        };
        let mut provider = MemoryConfig::new(cfg);
        provider.get_mut().port = 25567;
        assert_eq!(provider.get().port, 25567);
    }

    #[test]
    fn test_memory_config_persist() {
        let cfg = TestConfig::default();
        let provider = MemoryConfig::new(cfg);
        assert!(provider.persist().is_ok());
    }

    #[test]
    fn test_config_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let ce: ConfigError = io_err.into();
        assert!(matches!(ce, ConfigError::Io(_)));
    }
}
