use crate::minecraft::version::{LibraryInfo, Rule};
use crate::platform::{current_arch, current_os_name};
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
        let natives = self.natives.as_ref()?;
        let os_key = if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "macos") {
            "osx"
        } else {
            "linux"
        };

        let base = natives.get(os_key)?.clone();

        // Substitute ${arch} placeholder
        let arch = if cfg!(target_arch = "x86_64") {
            "64"
        } else if cfg!(target_arch = "x86") {
            "32"
        } else {
            ""
        };

        Some(base.replace("${{arch}}", arch))
    }

    /// Returns true if this library should be included for the current platform.
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        self.rules
            .as_ref()
            .is_none_or(|rules| evaluate_rules(rules))
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

        // Native artifacts use the classifier in the path
        let parts: Vec<&str> = self.name.split(':').collect();
        if parts.len() < 3 {
            return None;
        }

        let group = parts[0].replace('.', "/");
        let artifact_name = parts[1];
        let version = parts[2];

        Some(libraries_dir.join(format!(
            "{group}/{artifact_name}/{version}/{artifact_name}-{version}-{classifier}.jar"
        )))
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
    let mut allowed = false;

    for rule in rules {
        let applies = rule.os.as_ref().is_none_or(check_os_condition);

        if applies {
            allowed = rule.action == "allow";
        }
    }

    allowed
}

fn check_os_condition(os: &crate::minecraft::version::OsCondition) -> bool {
    if let Some(name) = &os.name
        && name != current_os_name()
    {
        return false;
    }

    if let Some(arch) = &os.arch
        && arch != current_arch()
    {
        return false;
    }

    true
}

/// Converts a Maven-style library name to a path.
///
/// E.g., "org.lwjgl:lwjgl:3.3.2" → "org/lwjgl/lwjgl/3.3.2/lwjgl-3.3.2.jar"
#[must_use]
pub fn artifact_path(name: &str) -> PathBuf {
    let parts: Vec<&str> = name.split(':').collect();
    if parts.len() < 3 {
        return PathBuf::from(name);
    }

    let group = parts[0].replace('.', "/");
    let artifact = parts[1];
    let version = parts[2];

    PathBuf::from(format!(
        "{group}/{artifact}/{version}/{artifact}-{version}.jar"
    ))
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
            rules: None,
            ..Default::default()
        };
        assert!(lib.is_allowed());
    }

    #[test]
    fn test_library_is_allowed_with_rules() {
        let lib = LibraryInfo {
            name: "test:lib:1.0".into(),
            rules: Some(vec![crate::minecraft::version::Rule {
                action: "allow".into(),
                os: Some(OsCondition {
                    name: Some(current_os_name().into()),
                    version: None,
                    arch: None,
                }),
                features: None,
            }]),
            ..Default::default()
        };
        assert!(lib.is_allowed());
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
