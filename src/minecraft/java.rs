pub mod discovery;
pub mod runtime;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Information about a Java runtime installation.
///
/// Mirrors SJMCL's `JavaInfo` structure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct JavaRuntime {
    /// Human-readable name (e.g., "JDK 21.0.2").
    pub name: String,
    /// Path to the java executable.
    pub exec_path: PathBuf,
    /// Java vendor.
    pub vendor: String,
    /// Major version number.
    pub major_version: i32,
    /// Whether this is an LTS release.
    pub is_lts: bool,
    /// Whether this was manually added by the user.
    pub is_user_added: bool,
}

/// Selector for choosing the appropriate Java runtime.
///
/// Inspired by SJMCL's `select_java_runtime` function.
pub struct JavaSelector {
    runtimes: Vec<JavaRuntime>,
}

impl JavaSelector {
    #[must_use]
    pub const fn new(runtimes: Vec<JavaRuntime>) -> Self {
        Self { runtimes }
    }

    /// Selects the best Java runtime for the given version requirement.
    ///
    /// # Arguments
    /// * `preferred` - Optional preferred runtime path.
    /// * `required_major` - The required Java major version (e.g., 17, 21).
    #[must_use]
    pub fn select(&self, preferred: Option<&str>, required_major: i32) -> Option<&JavaRuntime> {
        let paths: Vec<String> = self
            .runtimes
            .iter()
            .map(|runtime| runtime.exec_path.to_string_lossy().into_owned())
            .collect();
        let candidates: Vec<(&str, i32)> = paths
            .iter()
            .zip(&self.runtimes)
            .map(|(path, runtime)| (path.as_str(), runtime.major_version))
            .collect();
        select_runtime_index(&candidates, preferred, required_major)
            .map(|index| &self.runtimes[index])
    }

    /// Adds a runtime to the selector.
    pub fn add_runtime(&mut self, runtime: JavaRuntime) {
        self.runtimes.push(runtime);
    }

    /// Returns all known runtimes.
    #[must_use]
    pub fn runtimes(&self) -> &[JavaRuntime] {
        &self.runtimes
    }
}

impl Default for JavaSelector {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

/// Minimum requirements ordered newest first; manifest chronology handles snapshot names.
pub const JAVA_THRESHOLDS: &[(&str, i32)] = &[
    ("26.1-snapshot-1", 25),
    ("24w14a", 21),
    ("1.18-pre2", 17),
    ("21w19a", 16),
    ("17w13a", 8),
];
#[must_use]
pub fn loader_java_requirement(
    loader: crate::minecraft::loaders::ModLoaderType,
    version: &str,
) -> i32 {
    if loader != crate::minecraft::loaders::ModLoaderType::Cleanroom {
        return 0;
    }
    let threshold = semver::Version {
        major: 0,
        minor: 5,
        patch: 0,
        pre: semver::Prerelease::new("alpha").unwrap_or_default(),
        build: semver::BuildMetadata::EMPTY,
    };
    if semver::Version::parse(version).is_ok_and(|version| version < threshold) {
        21
    } else {
        25
    }
}
/// Returns the selected entry's index, preserving order for equal Java versions.
#[must_use]
pub fn select_runtime_index(
    runtimes: &[(&str, i32)],
    preferred: Option<&str>,
    required: i32,
) -> Option<usize> {
    if let Some(preferred) = preferred {
        return runtimes.iter().position(|(path, _)| *path == preferred);
    }
    runtimes
        .iter()
        .enumerate()
        .filter(|(_, (_, major))| *major >= required)
        .min_by_key(|(_, (_, major))| *major)
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_runtime(name: &str, major: i32) -> JavaRuntime {
        JavaRuntime {
            name: name.into(),
            exec_path: PathBuf::from(format!("/usr/lib/jvm/{name}/bin/java")),
            vendor: "Oracle".into(),
            major_version: major,
            is_lts: major == 17 || major == 21,
            is_user_added: false,
        }
    }

    #[test]
    fn test_java_selector() {
        let runtimes = vec![
            make_runtime("java-8", 8),
            make_runtime("java-17", 17),
            make_runtime("java-21", 21),
        ];

        let selector = JavaSelector::new(runtimes);

        // Should select Java 17 for requirement 17
        let selected = selector.select(None, 17);
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().major_version, 17);

        // Should select Java 21 for requirement 21
        let selected = selector.select(None, 21);
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().major_version, 21);

        // Should select Java 17 for requirement 11 (minimum that satisfies)
        let selected = selector.select(None, 11);
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().major_version, 17);
    }

    #[test]
    fn test_java_selector_preferred() {
        let runtimes = vec![make_runtime("java-17", 17), make_runtime("java-21", 21)];

        let selector = JavaSelector::new(runtimes);

        let preferred = "/usr/lib/jvm/java-21/bin/java";
        let selected = selector.select(Some(preferred), 17);
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().name, "java-21");
    }

    #[test]
    fn test_java_selector_no_match() {
        let runtimes = vec![make_runtime("java-8", 8)];
        let selector = JavaSelector::new(runtimes);
        assert!(selector.select(None, 17).is_none());
    }

    #[test]
    fn test_java_selector_add_runtime() {
        let mut selector = JavaSelector::new(vec![]);
        selector.add_runtime(make_runtime("java-17", 17));
        assert_eq!(selector.runtimes().len(), 1);
    }

    #[test]
    fn test_java_selector_default() {
        let selector = JavaSelector::default();
        assert!(selector.runtimes().is_empty());
    }
}
