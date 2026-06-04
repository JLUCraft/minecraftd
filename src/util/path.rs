use std::path::{Component, Path, PathBuf};

/// Errors that can occur during path operations.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("path traversal detected: {0}")]
    PathTraversal(String),
    #[error("absolute paths are not allowed: {0}")]
    AbsolutePath(String),
    #[error("invalid path: {0}")]
    InvalidPath(String),
}

/// Normalizes a relative path, rejecting any path traversal attempts.
///
/// This prevents directory traversal attacks by rejecting paths containing `..`
/// or absolute path components.
///
/// Inspired by SJMCL's `normalize_relative_path`.
pub fn normalize_relative_path(path: &str) -> Result<PathBuf, PathError> {
    let path = Path::new(path);
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Normal(part) => {
                normalized.push(part);
            }
            Component::ParentDir => {
                return Err(PathError::PathTraversal(path.to_string_lossy().to_string()));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(PathError::AbsolutePath(path.to_string_lossy().to_string()));
            }
            Component::CurDir => {
                // Skip `.` components
            }
        }
    }

    Ok(normalized)
}

/// Validates that a path is safe for filesystem operations.
///
/// Checks:
/// - No path traversal (`..`)
/// - No absolute paths
/// - No null bytes
pub fn validate_path_safety(path: &str) -> Result<(), PathError> {
    if path.contains('\0') {
        return Err(PathError::InvalidPath("null byte detected".to_string()));
    }
    normalize_relative_path(path)?;
    Ok(())
}

/// Resolves a path relative to a base directory, ensuring the result stays within the base.
pub fn resolve_within(base: &Path, relative: &str) -> Result<PathBuf, PathError> {
    let normalized = normalize_relative_path(relative)?;
    let resolved = base.join(normalized);

    // Ensure the resolved path is still within the base directory
    let canonical_base = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
    let canonical_resolved = resolved.canonicalize().unwrap_or_else(|_| resolved.clone());

    if !canonical_resolved.starts_with(&canonical_base) {
        return Err(PathError::PathTraversal(relative.to_string()));
    }

    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_valid() {
        assert_eq!(
            normalize_relative_path("foo/bar/baz").unwrap(),
            PathBuf::from("foo/bar/baz")
        );
        assert_eq!(
            normalize_relative_path("./foo/./bar").unwrap(),
            PathBuf::from("foo/bar")
        );
    }

    #[test]
    fn test_normalize_traversal() {
        assert!(normalize_relative_path("foo/../bar").is_err());
        assert!(normalize_relative_path("../foo").is_err());
    }

    #[test]
    fn test_normalize_absolute() {
        #[cfg(unix)]
        assert!(normalize_relative_path("/foo/bar").is_err());
    }

    #[test]
    fn test_validate_path_safety() {
        assert!(validate_path_safety("versions/1.20.4").is_ok());
        assert!(validate_path_safety("versions/../etc").is_err());
        assert!(validate_path_safety("foo\0bar").is_err());
    }

    #[test]
    fn test_resolve_within() {
        let base = std::env::temp_dir()
            .canonicalize()
            .unwrap_or_else(|_| std::env::temp_dir());
        let resolved = resolve_within(&base, "foo/bar").unwrap();
        assert!(resolved.starts_with(&base));
    }

    #[test]
    fn test_resolve_within_current_dir() {
        let base = std::env::temp_dir().join("minecraftd_test_resolve");
        let _ = std::fs::create_dir_all(&base);
        let base_canon = base.canonicalize().unwrap_or_else(|_| base.clone());
        let resolved = resolve_within(&base_canon, "foo/bar").unwrap();
        assert!(resolved.starts_with(&base_canon));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_resolve_within_traversal() {
        let base = std::env::temp_dir();
        let result = resolve_within(&base, "../outside");
        assert!(result.is_err());
    }
}
