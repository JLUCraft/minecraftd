use crate::minecraft::version::{LibraryInfo, Rule};
#[cfg(test)]
use crate::platform::current_os_name;
use std::path::{Path, PathBuf};

impl LibraryInfo {
    /// Returns true if this library is a native library for the current platform.
    #[must_use]
    pub const fn is_native(&self) -> bool {
        self.natives.is_some()
    }

    /// Returns the native classifier for the current platform.
    ///
    /// E.g., "natives-osx-arm64", "natives-linux", "natives-windows-x86_64"
    #[must_use]
    pub fn native_classifier(&self) -> Option<String> {
        crate::minecraft::dependencies::native_classifier(
            self.natives.as_ref()?,
            &Default::default(),
        )
    }

    /// Returns true if this library should be included for the current platform.
    #[must_use]
    pub fn allowed_for_current_platform(&self) -> bool {
        evaluate_rules(&self.rules)
    }

    /// Returns the local path where this library should be stored.
    #[must_use]
    pub fn local_path(&self, libraries_dir: &Path) -> PathBuf {
        libraries_dir.join(artifact_path(&self.name))
    }

    /// Returns the local path for the native artifact.
    #[must_use]
    pub fn native_path(&self, libraries_dir: &Path) -> Option<PathBuf> {
        let classifier = self.native_classifier()?;
        let downloads = self.downloads.as_ref()?;
        let classifiers = downloads.classifiers.as_ref()?;
        let _artifact = classifiers.get(&classifier)?;

        crate::minecraft::dependencies::convert_library_name_to_path(&self.name, Some(classifier))
            .ok()
            .map(|path| libraries_dir.join(path))
    }

    /// Returns the `DownloadInfo` for the main artifact.
    #[must_use]
    pub fn artifact_download(&self) -> Option<&crate::minecraft::version::DownloadInfo> {
        self.downloads.as_ref()?.artifact.as_ref()
    }

    /// Returns the `DownloadInfo` for the native artifact.
    #[must_use]
    pub fn native_download(&self) -> Option<&crate::minecraft::version::DownloadInfo> {
        let classifier = self.native_classifier()?;
        self.downloads
            .as_ref()?
            .classifiers
            .as_ref()?
            .get(&classifier)
    }
}

/// Evaluates a list of rules to determine if a library/argument is allowed.
#[must_use]
pub fn evaluate_rules(rules: &[Rule]) -> bool {
    crate::minecraft::profile::rules_is_allowed(rules, &Default::default()).unwrap_or(false)
}

/// Converts a Maven-style library name to a path.
///
/// E.g., "org.lwjgl:lwjgl:3.3.2" → "org/lwjgl/lwjgl/3.3.2/lwjgl-3.3.2.jar"
#[must_use]
pub fn artifact_path(name: &str) -> PathBuf {
    crate::minecraft::dependencies::convert_library_name_to_path(name, None)
        .map_or_else(|_| PathBuf::from(name), PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minecraft::version::OsCondition;

    #[test]
    fn test_library_is_native() {
        let lib = LibraryInfo {
            name: "org.lwjgl:lwjgl:3.3.2".into(),
            natives: Some({
                let mut m = std::collections::HashMap::new();
                m.insert("osx".into(), "natives-macos".into());
                m
            }),
            ..Default::default()
        };
        assert!(lib.is_native());
    }

    #[test]
    fn test_library_is_allowed_no_rules() {
        let lib = LibraryInfo {
            name: "test:lib:1.0".into(),
            rules: Vec::new(),
            ..Default::default()
        };
        assert!(lib.allowed_for_current_platform());
    }

    #[test]
    fn test_library_is_allowed_with_rules() {
        let lib = LibraryInfo {
            name: "test:lib:1.0".into(),
            rules: vec![crate::minecraft::version::Rule {
                action: "allow".into(),
                os: Some(OsCondition {
                    name: current_os_name().into(),
                    version: None,
                    arch: None,
                }),
                features: None,
            }],
            ..Default::default()
        };
        assert!(lib.allowed_for_current_platform());
    }

    #[test]
    fn test_library_local_path() {
        let lib = LibraryInfo {
            name: "org.lwjgl:lwjgl:3.3.2".into(),
            ..Default::default()
        };
        let path = lib.local_path(Path::new("/libs"));
        assert!(path.to_string_lossy().contains("org/lwjgl/lwjgl/3.3.2"));
    }

    #[test]
    fn test_artifact_path() {
        assert_eq!(
            artifact_path("org.lwjgl:lwjgl:3.3.2"),
            PathBuf::from("org/lwjgl/lwjgl/3.3.2/lwjgl-3.3.2.jar")
        );
    }
}
