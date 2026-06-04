use crate::minecraft::version::VersionInfo;
use crate::util::hash::sha1_file;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Result of a file validation check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationResult {
    /// Files that are valid (correct hash/size).
    pub valid: Vec<PathBuf>,
    /// Files that are missing.
    pub missing: Vec<FileRequirement>,
    /// Files that have incorrect hashes.
    pub corrupted: Vec<(PathBuf, FileRequirement)>,
    /// Files that have incorrect sizes.
    pub wrong_size: Vec<(PathBuf, FileRequirement)>,
}

impl ValidationResult {
    /// Returns true if all files are valid.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.missing.is_empty() && self.corrupted.is_empty() && self.wrong_size.is_empty()
    }

    /// Returns all files that need to be re-downloaded.
    #[must_use]
    pub fn needs_download(&self) -> Vec<&FileRequirement> {
        let mut needs = Vec::new();
        needs.extend(self.missing.iter());
        needs.extend(self.corrupted.iter().map(|(_, req)| req));
        needs.extend(self.wrong_size.iter().map(|(_, req)| req));
        needs
    }
}

/// A file that needs to be present with specific attributes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileRequirement {
    /// Local path where the file should exist.
    pub path: PathBuf,
    /// Expected SHA1 hash.
    pub sha1: Option<String>,
    /// Expected file size in bytes.
    pub size: Option<u64>,
    /// URL to download from if missing/corrupted.
    pub url: Option<String>,
}

/// Checks a file's existence, size, and SHA1. Returns `None` if valid,
/// or `Some(FileRequirement)` with the expected attributes if not.
async fn check_file(
    path: &Path,
    sha1: Option<&str>,
    size: Option<u64>,
    url: Option<&str>,
) -> Option<FileRequirement> {
    let req = || FileRequirement {
        path: path.to_path_buf(),
        sha1: sha1.map(std::string::ToString::to_string),
        size,
        url: url.map(std::string::ToString::to_string),
    };
    if !path.exists() {
        return Some(req());
    }
    if let Some(expected) = size
        && tokio::fs::metadata(path).await.map_or(0, |m| m.len()) != expected
    {
        return Some(req());
    }
    if let Some(expected) = sha1 {
        match sha1_file(path).await {
            Ok(actual) if actual != expected => return Some(req()),
            Err(_) => return Some(req()),
            _ => {}
        }
    }
    None
}

/// Validates game libraries against the version info.
///
/// Checks: existence, size, SHA1 hash.
pub async fn validate_libraries(
    version_info: &VersionInfo,
    libraries_dir: &Path,
) -> ValidationResult {
    let mut valid = Vec::new();
    let mut missing = Vec::new();
    let mut corrupted = Vec::new();
    let mut wrong_size = Vec::new();

    for lib in &version_info.libraries {
        if let Some(rules) = &lib.rules
            && !crate::minecraft::library::evaluate_rules(rules)
        {
            continue;
        }
        if let Some(downloads) = &lib.downloads
            && let Some(artifact) = &downloads.artifact
        {
            let path = libraries_dir.join(crate::minecraft::library::artifact_path(&lib.name));
            if let Some(req) = check_file(
                &path,
                Some(&artifact.sha1),
                Some(artifact.size),
                Some(&artifact.url),
            )
            .await
            {
                if !path.exists() {
                    missing.push(req);
                } else if tokio::fs::metadata(&path).await.map_or(0, |m| m.len()) != artifact.size {
                    wrong_size.push((path, req));
                } else {
                    corrupted.push((path, req));
                }
            } else {
                valid.push(path);
            }
        }
    }

    ValidationResult {
        valid,
        missing,
        corrupted,
        wrong_size,
    }
}

/// Validates game assets against the asset index.
///
/// Checks: existence, size, SHA1 hash.
pub async fn validate_assets(
    asset_index: &HashMap<String, AssetEntry>,
    assets_dir: &Path,
) -> ValidationResult {
    let mut valid = Vec::new();
    let mut missing = Vec::new();
    let mut corrupted = Vec::new();
    let mut wrong_size = Vec::new();

    for entry in asset_index.values() {
        let path = assets_dir
            .join("objects")
            .join(&entry.hash[..2])
            .join(&entry.hash);
        if let Some(req) = check_file(&path, Some(&entry.hash), Some(entry.size), None).await {
            if !path.exists() {
                missing.push(req);
            } else if tokio::fs::metadata(&path).await.map_or(0, |m| m.len()) != entry.size {
                wrong_size.push((path, req));
            } else {
                corrupted.push((path, req));
            }
        } else {
            valid.push(path);
        }
    }

    ValidationResult {
        valid,
        missing,
        corrupted,
        wrong_size,
    }
}

/// An entry in the asset index.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssetEntry {
    pub hash: String,
    pub size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minecraft::version::{
        DownloadInfo, LibraryDownloads, LibraryInfo, OsCondition, Rule,
    };

    #[test]
    fn test_validation_result() {
        let result = ValidationResult {
            valid: vec![PathBuf::from("a.jar")],
            missing: vec![],
            corrupted: vec![],
            wrong_size: vec![],
        };
        assert!(result.is_valid());

        let result = ValidationResult {
            valid: vec![],
            missing: vec![FileRequirement {
                path: PathBuf::from("b.jar"),
                sha1: None,
                size: None,
                url: None,
            }],
            corrupted: vec![],
            wrong_size: vec![],
        };
        assert!(!result.is_valid());
    }

    #[test]
    fn test_validation_result_needs_download() {
        let req = FileRequirement {
            path: PathBuf::from("x.jar"),
            sha1: Some("abc".into()),
            size: Some(100),
            url: Some("http://example.com/x.jar".into()),
        };
        let result = ValidationResult {
            valid: vec![],
            missing: vec![req],
            corrupted: vec![],
            wrong_size: vec![],
        };
        let needs = result.needs_download();
        assert_eq!(needs.len(), 1);
    }

    #[tokio::test]
    async fn test_validate_libraries_missing_and_wrong_size() {
        let tmpdir = tempfile::tempdir().unwrap();
        let lib_dir = tmpdir.path().join("libraries");
        std::fs::create_dir_all(&lib_dir).unwrap();

        // Create a file with wrong size
        let artifact_path = lib_dir.join("com/mojang/authlib/4.0.43/authlib-4.0.43.jar");
        std::fs::create_dir_all(artifact_path.parent().unwrap()).unwrap();
        std::fs::write(&artifact_path, b"short").unwrap(); // 5 bytes, expected 100

        let version_info = VersionInfo {
            libraries: vec![
                LibraryInfo {
                    name: "com.mojang:authlib:4.0.43".into(),
                    downloads: Some(LibraryDownloads {
                        artifact: Some(DownloadInfo {
                            sha1: "abc".into(),
                            size: 100,
                            url: "http://example.com/authlib.jar".into(),
                        }),
                        classifiers: None,
                    }),
                    rules: None,
                    ..Default::default()
                },
                LibraryInfo {
                    name: "org.missing:lib:1.0.0".into(),
                    downloads: Some(LibraryDownloads {
                        artifact: Some(DownloadInfo {
                            sha1: "def".into(),
                            size: 50,
                            url: "http://example.com/missing.jar".into(),
                        }),
                        classifiers: None,
                    }),
                    rules: None,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let result = validate_libraries(&version_info, &lib_dir).await;
        assert!(!result.is_valid());
        assert_eq!(result.missing.len(), 1);
        assert_eq!(result.wrong_size.len(), 1);
        assert_eq!(result.valid.len(), 0);
    }

    #[tokio::test]
    async fn test_validate_libraries_skipped_by_rules() {
        let tmpdir = tempfile::tempdir().unwrap();
        let lib_dir = tmpdir.path().join("libraries");

        let version_info = VersionInfo {
            libraries: vec![LibraryInfo {
                name: "windows:only:1.0".into(),
                downloads: Some(LibraryDownloads {
                    artifact: Some(DownloadInfo {
                        sha1: "abc".into(),
                        size: 10,
                        url: "http://example.com/win.jar".into(),
                    }),
                    classifiers: None,
                }),
                rules: Some(vec![Rule {
                    action: "allow".into(),
                    os: Some(OsCondition {
                        name: Some("windows".into()),
                        ..Default::default()
                    }),
                    features: None,
                }]),
                ..Default::default()
            }],
            ..Default::default()
        };

        let result = validate_libraries(&version_info, &lib_dir).await;
        // Library is allowed only on Windows; on other platforms it's skipped.
        if cfg!(target_os = "windows") {
            assert_eq!(result.missing.len(), 1);
        } else {
            assert!(result.is_valid());
        }
    }

    #[tokio::test]
    async fn test_validate_assets_missing() {
        let tmpdir = tempfile::tempdir().unwrap();
        let assets_dir = tmpdir.path().join("assets");

        let mut asset_index = HashMap::new();
        asset_index.insert(
            "icon.png".into(),
            AssetEntry {
                hash: "aabbccdd".into(),
                size: 1024,
            },
        );

        let result = validate_assets(&asset_index, &assets_dir).await;
        assert!(!result.is_valid());
        assert_eq!(result.missing.len(), 1);
    }

    #[tokio::test]
    async fn test_validate_assets_wrong_size() {
        let tmpdir = tempfile::tempdir().unwrap();
        let assets_dir = tmpdir.path().join("assets");
        let obj_dir = assets_dir.join("objects").join("aa");
        std::fs::create_dir_all(&obj_dir).unwrap();
        let path = obj_dir.join("aabbccdd");
        std::fs::write(&path, b"too small").unwrap();

        let mut asset_index = HashMap::new();
        asset_index.insert(
            "icon.png".into(),
            AssetEntry {
                hash: "aabbccdd".into(),
                size: 1024,
            },
        );

        let result = validate_assets(&asset_index, &assets_dir).await;
        assert!(!result.is_valid());
        assert_eq!(result.wrong_size.len(), 1);
    }

    #[tokio::test]
    async fn test_validate_assets_valid() {
        let tmpdir = tempfile::tempdir().unwrap();
        let assets_dir = tmpdir.path().join("assets");
        let obj_dir = assets_dir.join("objects").join("60");
        std::fs::create_dir_all(&obj_dir).unwrap();
        let content = vec![0u8; 1024];
        let hash = crate::util::hash::sha1_bytes(&content);
        let path = obj_dir.join(&hash);
        std::fs::write(&path, &content).unwrap();

        let mut asset_index = HashMap::new();
        asset_index.insert("icon.png".into(), AssetEntry { hash, size: 1024 });

        let result = validate_assets(&asset_index, &assets_dir).await;
        assert!(result.is_valid());
        assert_eq!(result.valid.len(), 1);
    }

    #[tokio::test]
    async fn test_validate_assets_sha1_mismatch() {
        let tmpdir = tempfile::tempdir().unwrap();
        let assets_dir = tmpdir.path().join("assets");
        let obj_dir = assets_dir.join("objects").join("aa");
        std::fs::create_dir_all(&obj_dir).unwrap();
        let path = obj_dir.join("aabbccdd");
        std::fs::write(&path, b"wrong content").unwrap();

        let mut asset_index = HashMap::new();
        asset_index.insert(
            "icon.png".into(),
            AssetEntry {
                hash: "aabbccdd".into(),
                size: 13,
            },
        );

        let result = validate_assets(&asset_index, &assets_dir).await;
        assert!(!result.is_valid());
        assert_eq!(result.corrupted.len(), 1);
    }

    #[tokio::test]
    async fn test_validate_libraries_sha1_mismatch() {
        let tmpdir = tempfile::tempdir().unwrap();
        let lib_dir = tmpdir.path().join("libraries");
        let artifact_path = lib_dir.join("com/mojang/authlib/4.0.43/authlib-4.0.43.jar");
        std::fs::create_dir_all(artifact_path.parent().unwrap()).unwrap();
        std::fs::write(&artifact_path, b"wrong content").unwrap();

        let version_info = VersionInfo {
            libraries: vec![LibraryInfo {
                name: "com.mojang:authlib:4.0.43".into(),
                downloads: Some(LibraryDownloads {
                    artifact: Some(DownloadInfo {
                        sha1: "aabbccdd".into(),
                        size: 13,
                        url: "http://example.com/authlib.jar".into(),
                    }),
                    classifiers: None,
                }),
                rules: None,
                ..Default::default()
            }],
            ..Default::default()
        };

        let result = validate_libraries(&version_info, &lib_dir).await;
        assert!(!result.is_valid());
        assert_eq!(result.corrupted.len(), 1);
    }
}
