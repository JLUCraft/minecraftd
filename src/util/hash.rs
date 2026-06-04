use sha1::Digest;
use std::io;
use std::path::Path;
use thiserror::Error;

/// Errors that can occur during hash operations.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum HashError {
    #[error("io error: {0}")]
    Io(String),
    #[error("sha1 mismatch: expected {expected}, got {actual}")]
    Sha1Mismatch { expected: String, actual: String },
}

impl From<io::Error> for HashError {
    fn from(e: io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Computes the SHA1 hash of raw bytes.
#[must_use]
pub fn sha1_bytes(data: &[u8]) -> String {
    let mut hasher = sha1::Sha1::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Computes the SHA1 hash of a file.
pub async fn sha1_file(path: &Path) -> Result<String, HashError> {
    let content = tokio::fs::read(path).await?;
    Ok(sha1_bytes(&content))
}

/// Verifies that a file's SHA1 matches the expected value.
pub async fn verify_sha1(path: &Path, expected: &str) -> Result<(), HashError> {
    let actual = sha1_file(path).await?;
    if actual != expected {
        return Err(HashError::Sha1Mismatch {
            expected: expected.to_string(),
            actual,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sha1_file() {
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().join("test.txt");
        tokio::fs::write(&path, b"hello").await.unwrap();

        let hash = sha1_file(&path).await.unwrap();
        // SHA1 of "hello"
        assert_eq!(hash, "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d");
    }

    #[tokio::test]
    async fn test_verify_sha1_match() {
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().join("test.txt");
        tokio::fs::write(&path, b"hello").await.unwrap();

        assert!(
            verify_sha1(&path, "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d")
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn test_verify_sha1_mismatch() {
        let tmpdir = tempfile::tempdir().unwrap();
        let path = tmpdir.path().join("test.txt");
        tokio::fs::write(&path, b"hello").await.unwrap();

        let result = verify_sha1(&path, "0000000000000000000000000000000000000000").await;
        assert!(matches!(result, Err(HashError::Sha1Mismatch { .. })));
    }
}
