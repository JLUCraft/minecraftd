use crate::minecraft::loaders::ModLoaderType;
use crate::minecraft::version::VersionInfo;
use std::path::Path;
use thiserror::Error;

/// Errors that can occur during mod loader installation.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum LoaderInstallError {
    #[error("network error: {0}")]
    Network(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("unsupported loader: {0}")]
    Unsupported(String),
    #[error("version not found: {0}")]
    VersionNotFound(String),
    #[error("installation failed: {0}")]
    InstallFailed(String),
}

impl From<std::io::Error> for LoaderInstallError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<reqwest::Error> for LoaderInstallError {
    fn from(e: reqwest::Error) -> Self {
        Self::Network(e.to_string())
    }
}

impl From<serde_json::Error> for LoaderInstallError {
    fn from(e: serde_json::Error) -> Self {
        Self::Parse(e.to_string())
    }
}

/// Finds a suitable Java executable for running installer jars.
pub async fn find_java_executable() -> Result<std::path::PathBuf, LoaderInstallError> {
    // Try `java` from PATH first
    if let Ok(path) = which::which("java") {
        return Ok(path);
    }

    // Try common Java locations
    #[cfg(target_os = "macos")]
    let candidates = [
        "/usr/bin/java",
        "/Library/Java/JavaVirtualMachines/Contents/Home/bin/java",
        "/System/Library/Java/JavaVirtualMachines/Contents/Home/bin/java",
    ];

    #[cfg(target_os = "linux")]
    let candidates = [
        "/usr/bin/java",
        "/usr/lib/jvm/default-java/bin/java",
        "/usr/lib/jvm/default/bin/java",
    ];

    #[cfg(target_os = "windows")]
    let candidates = [
        r"C:\Program Files\Java\bin\java.exe",
        r"C:\Program Files (x86)\Java\bin\java.exe",
    ];

    for path in &candidates {
        let p = std::path::PathBuf::from(path);
        if p.exists() {
            return Ok(p);
        }
    }

    Err(LoaderInstallError::InstallFailed(
        "Could not find a Java installation. Please install Java and ensure it is in PATH."
            .to_string(),
    ))
}

/// Runs a Java jar installer and verifies it produced the expected output.
///
/// This is a shared helper used by Forge and `NeoForge` installers.
/// It is extracted so it can be unit-tested with a fake Java binary.
pub async fn run_installer_jar(
    java_path: &std::path::Path,
    installer_jar: &std::path::Path,
    game_dir: &std::path::Path,
) -> Result<(), LoaderInstallError> {
    let output = tokio::process::Command::new(java_path)
        .arg("-jar")
        .arg(installer_jar)
        .arg("--installServer")
        .arg(game_dir)
        .current_dir(game_dir)
        .output()
        .await
        .map_err(|e| LoaderInstallError::InstallFailed(format!("failed to run installer: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(LoaderInstallError::InstallFailed(format!(
            "installer exited with code {:?}. stdout: {} stderr: {}",
            output.status.code(),
            stdout,
            stderr
        )));
    }

    Ok(())
}

/// Trait for mod loader installers.
#[async_trait::async_trait]
pub trait ModLoaderInstaller: Send + Sync {
    /// Returns the loader type.
    fn loader_type(&self) -> ModLoaderType;

    /// Installs the mod loader for a given Minecraft version.
    ///
    /// Returns the modified `VersionInfo` with loader libraries merged.
    async fn install(
        &self,
        mc_version: &str,
        loader_version: &str,
        game_dir: &Path,
    ) -> Result<VersionInfo, LoaderInstallError>;
}
