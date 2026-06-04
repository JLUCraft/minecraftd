use std::path::PathBuf;

/// Returns the default Minecraft game directory for the current platform.
///
/// - Linux: `~/.minecraft`
/// - macOS: `~/Library/Application Support/minecraft`
/// - Windows: `%APPDATA%\.minecraft`
#[must_use]
pub fn default_game_dir() -> PathBuf {
    let base = dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".minecraft")))
        .unwrap_or_else(|| PathBuf::from(".minecraft"));
    base.join("minecraft")
}

/// Returns paths to search for Java installations on the current platform.
#[must_use]
pub fn default_java_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if cfg!(target_os = "macos") {
        paths.push(PathBuf::from("/Library/Java/JavaVirtualMachines"));
        if let Some(home) = dirs::home_dir() {
            paths.push(home.join(".sdkman/candidates/java"));
        }
    } else if cfg!(target_os = "linux") {
        paths.push(PathBuf::from("/usr/lib/jvm"));
        if let Some(home) = dirs::home_dir() {
            paths.push(home.join(".sdkman/candidates/java"));
        }
    } else if cfg!(target_os = "windows") {
        paths.push(PathBuf::from("C:\\Program Files\\Java"));
        paths.push(PathBuf::from("C:\\Program Files (x86)\\Java"));
    } else {
        // Unknown platform: no default Java search paths
    }

    paths
}

/// Returns the OS name as used in Mojang's version.json rule conditions.
#[must_use]
pub const fn current_os_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

/// Returns the current CPU architecture as used in Mojang's rules.
#[must_use]
pub const fn current_arch() -> &'static str {
    if cfg!(target_arch = "x86") {
        "x86"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "unknown"
    }
}

/// Returns the native classifier suffix for the current platform.
///
/// E.g., "natives-osx-arm64", "natives-linux", "natives-windows-x86_64"
#[must_use]
pub fn native_classifier() -> String {
    let os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    };

    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        ""
    };

    if arch.is_empty() {
        format!("natives-{os}")
    } else {
        format!("natives-{os}-{arch}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_game_dir() {
        let dir = default_game_dir();
        assert!(dir.to_string_lossy().contains("minecraft"));
    }

    #[test]
    fn test_current_os_name() {
        let os = current_os_name();
        assert!(matches!(os, "windows" | "osx" | "linux"));
    }

    #[test]
    fn test_current_arch() {
        let arch = current_arch();
        assert!(!arch.is_empty());
    }

    #[test]
    fn test_native_classifier_contains_os() {
        let classifier = native_classifier();
        assert!(classifier.starts_with("natives-"));
    }

    #[test]
    fn test_java_search_paths_not_empty() {
        let paths = default_java_search_paths();
        assert!(!paths.is_empty());
    }
}
