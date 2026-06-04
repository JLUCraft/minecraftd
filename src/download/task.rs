use std::time::Duration;

use crate::download::mirror::DownloadSource;
use thiserror::Error;
use tracing::{debug, info, warn};

/// Progress information for an ongoing download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

/// Errors that can occur during a download task.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    #[error("network error: {0}")]
    Network(String),
    #[error("http error {status}: {message}")]
    Http { status: u16, message: String },
    #[error("sha1 mismatch: expected {expected}, got {actual}")]
    Sha1Mismatch { expected: String, actual: String },
    #[error("io error: {0}")]
    Io(String),
    #[error("all sources failed")]
    AllSourcesFailed,
    #[error("retry exhausted after {attempts} attempts: {last_error}")]
    RetryExhausted { attempts: u32, last_error: String },
}

impl From<std::io::Error> for DownloadError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Configuration for download retry behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub backoff_base_secs: u64,
    pub max_backoff_secs: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            backoff_base_secs: 1,
            max_backoff_secs: 8,
        }
    }
}

/// A single download task: URL → local file.
#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub url: String,
    pub destination: std::path::PathBuf,
    pub expected_sha1: Option<String>,
    pub sources: Vec<DownloadSource>,
    pub retry_config: Option<RetryConfig>,
}

impl DownloadTask {
    /// Creates a new `DownloadTask`.
    #[must_use]
    pub fn new(url: String, destination: std::path::PathBuf) -> Self {
        Self {
            url,
            destination,
            expected_sha1: None,
            sources: vec![DownloadSource::Official],
            retry_config: None,
        }
    }

    /// Sets the expected SHA1 hash.
    #[must_use]
    pub fn with_sha1(mut self, sha1: String) -> Self {
        self.expected_sha1 = Some(sha1);
        self
    }

    /// Sets the download sources.
    #[must_use]
    pub fn with_sources(mut self, sources: Vec<DownloadSource>) -> Self {
        self.sources = sources;
        self
    }

    /// Sets the retry configuration.
    ///
    /// `max_attempts` is the total number of attempts (including the first).
    /// `backoff_base_secs` is the initial delay between retries in seconds.
    ///
    /// # Example
    ///
    /// ```
    /// use minecraftd::download::task::DownloadTask;
    ///
    /// let task = DownloadTask::new(
    ///     "https://example.com/file.jar".to_string(),
    ///     std::path::PathBuf::from("/tmp/file.jar"),
    /// )
    /// .with_retry(3, 1);
    /// ```
    #[must_use]
    pub const fn with_retry(mut self, max_attempts: u32, backoff_base_secs: u64) -> Self {
        self.retry_config = Some(RetryConfig {
            max_attempts,
            backoff_base_secs,
            max_backoff_secs: backoff_base_secs
                .saturating_mul(1_u64.wrapping_shl(max_attempts.saturating_sub(1))),
        });
        self
    }

    /// Executes the download, trying each source in order.
    ///
    /// # Errors
    ///
    /// Returns `DownloadError` if all sources fail, SHA1 verification fails, or I/O errors occur.
    pub async fn execute(&self) -> Result<(), DownloadError> {
        if let Some(parent) = self.destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut last_error = None;
        for source in &self.sources {
            let url = source.rewrite_url(&self.url);
            debug!("downloading from: {}", url);

            match self.download_from(&url).await {
                Ok(()) => {
                    info!("downloaded: {}", self.destination.display());
                    return Ok(());
                }
                Err(e) => {
                    warn!("source failed ({}): {}", url, e);
                    last_error = Some(e);
                }
            }
        }

        if let Some(err @ DownloadError::RetryExhausted { .. }) = last_error {
            return Err(err);
        }

        Err(DownloadError::AllSourcesFailed)
    }

    async fn download_from(&self, url: &str) -> Result<(), DownloadError> {
        let has_retry = self.retry_config.is_some();
        let max_attempts = self.retry_config.map_or(1, |c| c.max_attempts);
        let backoff_base = self.retry_config.map_or(1, |c| c.backoff_base_secs);
        let max_backoff = self.retry_config.map_or(1, |c| c.max_backoff_secs);

        let mut last_error = None;

        for attempt in 1..=max_attempts {
            match self.try_download_once(url).await {
                Ok(()) => return Ok(()),
                Err((error, retryable)) => {
                    if !retryable {
                        return Err(error);
                    }
                    if attempt == max_attempts {
                        if has_retry {
                            return Err(DownloadError::RetryExhausted {
                                attempts: max_attempts,
                                last_error: error.to_string(),
                            });
                        }
                        return Err(error);
                    }
                    let backoff = backoff_base.saturating_mul(1_u64.wrapping_shl(attempt - 1));
                    let backoff = backoff.min(max_backoff);
                    warn!(
                        "download attempt {}/{} failed for {}, retrying in {}s: {}",
                        attempt, max_attempts, url, backoff, error
                    );
                    tokio::time::sleep(Duration::from_secs(backoff)).await;
                    last_error = Some(error);
                }
            }
        }

        Err(DownloadError::RetryExhausted {
            attempts: max_attempts,
            last_error: last_error.map_or_else(|| "unknown".to_string(), |e| e.to_string()),
        })
    }

    async fn try_download_once(&self, url: &str) -> Result<(), (DownloadError, bool)> {
        let response = match reqwest::get(url).await {
            Ok(r) => r,
            Err(e) => {
                let retryable = e.is_timeout() || e.is_connect();
                return Err((DownloadError::Network(e.to_string()), retryable));
            }
        };

        let status = response.status();
        if !status.is_success() {
            let retryable = status.is_server_error();
            return Err((
                DownloadError::Http {
                    status: status.as_u16(),
                    message: status.to_string(),
                },
                retryable,
            ));
        }

        let bytes = match response.bytes().await {
            Ok(b) => b,
            Err(e) => {
                return Err((DownloadError::Network(e.to_string()), true));
            }
        };

        match tokio::fs::write(&self.destination, &bytes).await {
            Ok(()) => {}
            Err(e) => return Err((DownloadError::from(e), false)),
        }

        if let Some(expected) = &self.expected_sha1 {
            let actual = crate::util::hash::sha1_bytes(&bytes);
            if &actual != expected {
                let _ = tokio::fs::remove_file(&self.destination).await;
                return Err((
                    DownloadError::Sha1Mismatch {
                        expected: expected.clone(),
                        actual,
                    },
                    false,
                ));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[test]
    fn test_download_task_builder() {
        let task = DownloadTask::new(
            "https://example.com/file.jar".to_string(),
            std::path::PathBuf::from("/tmp/file.jar"),
        )
        .with_sha1("abc123".to_string());

        assert_eq!(task.url, "https://example.com/file.jar");
        assert_eq!(task.destination, std::path::PathBuf::from("/tmp/file.jar"));
        assert_eq!(task.expected_sha1, Some("abc123".to_string()));
        assert!(task.retry_config.is_none());
    }

    #[test]
    fn test_download_task_with_retry() {
        let task = DownloadTask::new(
            "https://example.com/file.jar".to_string(),
            std::path::PathBuf::from("/tmp/file.jar"),
        )
        .with_retry(3, 1);

        let config = task.retry_config.unwrap();
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.backoff_base_secs, 1);
        assert_eq!(config.max_backoff_secs, 4);
    }

    #[test]
    fn test_download_task_with_retry_single_attempt() {
        let task = DownloadTask::new(
            "https://example.com/file.jar".to_string(),
            std::path::PathBuf::from("/tmp/file.jar"),
        )
        .with_retry(1, 2);

        let config = task.retry_config.unwrap();
        assert_eq!(config.max_attempts, 1);
        assert_eq!(config.backoff_base_secs, 2);
        assert_eq!(config.max_backoff_secs, 2);
    }

    #[tokio::test]
    async fn test_download_task_unreachable_host() {
        let task = DownloadTask::new(
            "http://localhost:59999/invalid".to_string(),
            tempfile::tempdir().unwrap().path().join("test.jar"),
        );

        let result = task.execute().await;
        assert!(
            result.is_err(),
            "expected error for unreachable host, got {result:?}"
        );
    }

    #[tokio::test]
    async fn test_download_task_all_sources_failed() {
        let task = DownloadTask::new(
            "http://localhost:59999/file.jar".to_string(),
            tempfile::tempdir().unwrap().path().join("test.jar"),
        )
        .with_sources(vec![
            crate::download::mirror::DownloadSource::Official,
            crate::download::mirror::DownloadSource::Bmclapi,
        ]);

        let result = task.execute().await;
        assert!(
            matches!(result, Err(DownloadError::AllSourcesFailed)),
            "expected AllSourcesFailed when every source is unreachable, got {result:?}"
        );
    }

    #[tokio::test]
    async fn test_download_retry_exhausted_on_server_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let response = "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });

        let task = DownloadTask::new(
            format!("http://127.0.0.1:{port}/file.jar"),
            tempfile::tempdir().unwrap().path().join("file.jar"),
        )
        .with_retry(3, 1);

        let result = task.execute().await;
        assert!(
            matches!(
                result,
                Err(DownloadError::RetryExhausted { attempts: 3, .. })
            ),
            "expected RetryExhausted after 3 attempts, got {result:?}"
        );
    }
}
