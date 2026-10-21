use crate::download::task::DownloadTask;
use crate::minecraft::loader::installer::{LoaderInstallError, ModLoaderInstaller};
use crate::minecraft::loaders::ModLoaderType;
use crate::minecraft::version::VersionInfo;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;
use tracing::{debug, info};

const PAPER_FILL_BASE: &str = "https://fill.papermc.io/v3/projects/paper";
const PAPER_USER_AGENT: &str = "JLUCraft-minecraftd/1.0 (https://github.com/JLUCraft/minecraftd)";
const PAPER_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const PAPER_FETCH_BACKOFF: Duration = Duration::from_millis(500);
const PAPER_MAX_ATTEMPTS: u32 = 3;
const PAPER_MAX_JAR_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct PaperBuild {
    id: u64,
    channel: PaperChannel,
    downloads: BTreeMap<String, PaperDownload>,
}
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum PaperChannel {
    Stable,
    #[serde(other)]
    Other,
}
#[derive(Debug, Deserialize)]
struct PaperDownload {
    url: String,
    size: u64,
    checksums: PaperChecksums,
}
#[derive(Debug, Deserialize)]
struct PaperChecksums {
    sha256: String,
}

fn paper_client() -> Result<reqwest::Client, LoaderInstallError> {
    Ok(reqwest::Client::builder()
        .user_agent(PAPER_USER_AGENT)
        .timeout(PAPER_REQUEST_TIMEOUT)
        .build()?)
}

/// Runs `operation` up to `PAPER_MAX_ATTEMPTS` times with linear backoff.
/// Upstream occasionally returns corrupted bodies under load ("error decoding
/// response body"), so transient transfer failures are retried before giving
/// up with the attempt count in the error message.
async fn with_retry<T, Fut>(
    context: &str,
    mut operation: impl FnMut() -> Fut,
) -> Result<T, LoaderInstallError>
where
    Fut: std::future::Future<Output = Result<T, LoaderInstallError>>,
{
    let mut last_error: Option<LoaderInstallError> = None;
    for attempt in 1..=PAPER_MAX_ATTEMPTS {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                if attempt < PAPER_MAX_ATTEMPTS {
                    tracing::warn!(
                        "{context} failed (attempt {attempt}/{PAPER_MAX_ATTEMPTS}): {error}"
                    );
                    tokio::time::sleep(PAPER_FETCH_BACKOFF * attempt).await;
                }
                last_error = Some(error);
            }
        }
    }
    let last = last_error.expect("loop runs at least once");
    let context = format!("{context} failed after {PAPER_MAX_ATTEMPTS} attempts");
    Err(match last {
        LoaderInstallError::Network(message) => {
            LoaderInstallError::Network(format!("{context}: {message}"))
        }
        LoaderInstallError::Parse(message) => {
            LoaderInstallError::Parse(format!("{context}: {message}"))
        }
        LoaderInstallError::InstallFailed(message) => {
            LoaderInstallError::InstallFailed(format!("{context}: {message}"))
        }
        other => other,
    })
}

/// Fetches a JSON document with bounded retries. Non-JSON payloads are
/// reported with a body snippet.
async fn fetch_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> Result<T, LoaderInstallError> {
    with_retry(&format!("Paper API request to {url}"), || {
        fetch_json_once::<T>(client, url)
    })
    .await
}

async fn fetch_json_once<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> Result<T, LoaderInstallError> {
    let body: String = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    serde_json::from_str(&body).map_err(|error| {
        LoaderInstallError::Parse(format!(
            "invalid JSON from {url}: {error}; body: {}",
            body.chars().take(200).collect::<String>()
        ))
    })
}
fn latest_stable(builds: &[PaperBuild]) -> Result<String, LoaderInstallError> {
    builds
        .iter()
        .filter(|build| build.channel == PaperChannel::Stable)
        .map(|build| build.id)
        .max()
        .map(|id| id.to_string())
        .ok_or_else(|| LoaderInstallError::VersionNotFound("no stable Paper build".to_string()))
}
fn verify_paper_bytes(download: &PaperDownload, bytes: &[u8]) -> Result<(), LoaderInstallError> {
    let checksum: String = hex::encode(Sha256::digest(bytes));
    if download.size != bytes.len() as u64 || download.checksums.sha256 != checksum {
        return Err(LoaderInstallError::InstallFailed(
            "Paper download size/SHA-256 mismatch".to_string(),
        ));
    }
    Ok(())
}

const PURPUR_API_BASE: &str = "https://api.purpurmc.org/v2";

/// Server platform supported by the Paper installer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaperPlatform {
    Paper,
    Purpur,
}

impl PaperPlatform {
    #[must_use]
    pub const fn project_name(&self) -> &'static str {
        match self {
            Self::Paper => "paper",
            Self::Purpur => "purpur",
        }
    }

    #[must_use]
    pub const fn display_name(&self) -> &'static str {
        match self {
            Self::Paper => "Paper",
            Self::Purpur => "Purpur",
        }
    }
}

/// Installer for Paper/Purpur server platforms.
///
/// Unlike Forge or Fabric, these platforms ship a single server JAR
/// that contains everything needed to run. No libraries or version JSON
/// patching is required.
#[derive(Debug, Clone)]
pub struct PaperInstaller {
    platform: PaperPlatform,
}

impl PaperInstaller {
    #[must_use]
    pub const fn new(platform: PaperPlatform) -> Self {
        Self { platform }
    }

    #[must_use]
    pub const fn paper() -> Self {
        Self::new(PaperPlatform::Paper)
    }

    #[must_use]
    pub const fn purpur() -> Self {
        Self::new(PaperPlatform::Purpur)
    }

    /// Download URL for the Purpur artifact listing. Paper itself ignores
    /// this: its artifact is resolved and verified through Fill v3 in
    /// `download_paper` instead.
    #[must_use]
    pub fn download_url(&self, mc_version: &str, build: &str) -> String {
        format!(
            "{}/{}/{}/builds/{}/downloads/{}-{}-{}.jar",
            PURPUR_API_BASE,
            self.platform.project_name(),
            mc_version,
            build,
            self.platform.project_name(),
            mc_version,
            build
        )
    }

    /// Fetches the latest successful build number for a Minecraft version.
    pub async fn fetch_latest_build(&self, mc_version: &str) -> Result<String, LoaderInstallError> {
        if self.platform == PaperPlatform::Paper {
            let url: String = format!(
                "{PAPER_FILL_BASE}/versions/{}/builds",
                urlencoding::encode(mc_version)
            );
            let builds: Vec<PaperBuild> = fetch_json(&paper_client()?, &url).await?;
            return latest_stable(&builds);
        }
        let url = format!(
            "{}/{}/{}",
            PURPUR_API_BASE,
            self.platform.project_name(),
            mc_version
        );

        debug!(
            "fetching {} builds from {}",
            self.platform.display_name(),
            url
        );
        let response = reqwest::get(&url).await?;
        if !response.status().is_success() {
            return Err(LoaderInstallError::Network(format!(
                "HTTP {} from {}",
                response.status(),
                url
            )));
        }

        let data: serde_json::Value = response.json().await?;

        // Purpur API: builds array with build numbers
        let builds = data["builds"]
            .as_array()
            .ok_or_else(|| LoaderInstallError::Parse("missing builds array".to_string()))?;

        if builds.is_empty() {
            return Err(LoaderInstallError::VersionNotFound(format!(
                "no builds found for {} {}",
                self.platform.display_name(),
                mc_version
            )));
        }

        // Get the last build (latest)
        let latest = builds
            .last()
            .and_then(serde_json::Value::as_i64)
            .or_else(|| builds.last().and_then(|b| b["build"].as_i64()))
            .ok_or_else(|| LoaderInstallError::Parse("invalid build number".to_string()))?;

        Ok(latest.to_string())
    }

    async fn download_paper(
        &self,
        mc_version: &str,
        build: &str,
        destination: &Path,
    ) -> Result<(), LoaderInstallError> {
        with_retry(&format!("Paper build {build} download"), || {
            self.download_paper_once(mc_version, build, destination)
        })
        .await
    }

    /// Downloads and verifies a single Paper build attempt. The destination
    /// file is only written after the checksum verifies, so a failed attempt
    /// never overwrites the installed jar and the whole step can be retried.
    async fn download_paper_once(
        &self,
        mc_version: &str,
        build: &str,
        destination: &Path,
    ) -> Result<(), LoaderInstallError> {
        let client: reqwest::Client = paper_client()?;
        let url: String = format!(
            "{PAPER_FILL_BASE}/versions/{}/builds/{}",
            urlencoding::encode(mc_version),
            urlencoding::encode(build)
        );
        let metadata: PaperBuild = fetch_json(&client, &url).await?;
        if metadata.id.to_string() != build {
            return Err(LoaderInstallError::Parse(
                "Paper build mismatch".to_string(),
            ));
        }
        let download: &PaperDownload =
            metadata.downloads.get("server:default").ok_or_else(|| {
                LoaderInstallError::Parse("missing Paper server download".to_string())
            })?;
        let parsed: url::Url = url::Url::parse(&download.url)
            .map_err(|error| LoaderInstallError::Parse(error.to_string()))?;
        if parsed.scheme() != "https"
            || parsed.host_str() != Some("fill-data.papermc.io")
            || download.size > PAPER_MAX_JAR_BYTES
        {
            return Err(LoaderInstallError::Parse(
                "invalid Paper artifact metadata".to_string(),
            ));
        }
        let mut response: reqwest::Response =
            client.get(parsed).send().await?.error_for_status()?;
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if (bytes.len() as u64).saturating_add(chunk.len() as u64) > download.size {
                return Err(LoaderInstallError::InstallFailed(
                    "Paper download exceeds declared size".to_string(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        verify_paper_bytes(download, &bytes)?;
        // Verify before touching the installed jar; failed upstream responses cannot overwrite it.
        tokio::fs::write(destination, &bytes).await?;
        Ok(())
    }

    /// Creates a synthetic `VersionInfo` for a Paper-based server.
    ///
    /// Paper servers don't use Mojang's version JSON — they are self-contained
    /// server JARs. We construct a minimal `VersionInfo` so the rest of the
    /// system can treat it uniformly.
    fn synthetic_version_info(&self, mc_version: &str, build: &str) -> VersionInfo {
        VersionInfo {
            id: format!("{}-{}-{}", self.platform.project_name(), mc_version, build),
            type_: "release".to_string(),
            main_class: None, // Server JARs use `java -jar`, not a main class
            libraries: vec![],
            // Mark as server-only by setting a known field
            ..Default::default()
        }
    }
}

#[async_trait::async_trait]
impl ModLoaderInstaller for PaperInstaller {
    fn loader_type(&self) -> ModLoaderType {
        ModLoaderType::Paper
    }

    async fn install(
        &self,
        mc_version: &str,
        loader_version: &str,
        game_dir: &Path,
    ) -> Result<VersionInfo, LoaderInstallError> {
        info!(
            "installing {} {} for Minecraft {}",
            self.platform.display_name(),
            loader_version,
            mc_version
        );

        // Ensure game directory exists
        tokio::fs::create_dir_all(game_dir).await?;

        // Determine build number: if loader_version looks like a build number, use it directly;
        // otherwise try to fetch the latest build.
        let build = if loader_version.parse::<u64>().is_ok() {
            loader_version.to_string()
        } else {
            self.fetch_latest_build(mc_version).await?
        };

        let server_jar = game_dir.join(format!(
            "{}-{}-{}.jar",
            self.platform.project_name(),
            mc_version,
            build
        ));

        if self.platform == PaperPlatform::Paper {
            self.download_paper(mc_version, &build, &server_jar).await?;
            return Ok(self.synthetic_version_info(mc_version, &build));
        }

        let download_url = self.download_url(mc_version, &build);
        debug!(
            "downloading {} server from {}",
            self.platform.display_name(),
            download_url
        );
        let task = DownloadTask::new(download_url.clone(), server_jar.clone());
        task.execute().await.map_err(|e| {
            LoaderInstallError::Network(format!("failed to download server jar: {e}"))
        })?;

        info!(
            "{} server {} installed to {}",
            self.platform.display_name(),
            build,
            server_jar.display()
        );

        Ok(self.synthetic_version_info(mc_version, &build))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_selection_and_artifact_integrity() -> Result<(), LoaderInstallError> {
        let builds: Vec<PaperBuild> = serde_json::from_str(
            r#"[
            {"id":10,"channel":"EXPERIMENTAL","downloads":{}},
            {"id":8,"channel":"STABLE","downloads":{}},
            {"id":9,"channel":"STABLE","downloads":{}}
        ]"#,
        )?;
        assert_eq!(latest_stable(&builds)?, "9");
        assert!(latest_stable(&builds[..1]).is_err());
        let bytes: &[u8] = b"verified server jar fixture";
        let metadata: PaperDownload = PaperDownload {
            url: String::new(),
            size: bytes.len() as u64,
            checksums: PaperChecksums {
                sha256: hex::encode(Sha256::digest(bytes)),
            },
        };
        verify_paper_bytes(&metadata, bytes)?;
        assert!(verify_paper_bytes(&metadata, b"altered server jar fixture!").is_err());
        assert!(verify_paper_bytes(&metadata, &bytes[..bytes.len() - 1]).is_err());
        Ok(())
    }

    #[test]
    fn test_paper_installer_platforms() {
        assert_eq!(PaperInstaller::paper().platform, PaperPlatform::Paper);
        assert_eq!(PaperInstaller::purpur().platform, PaperPlatform::Purpur);
    }

    #[test]
    fn test_purpur_download_url() {
        let installer = PaperInstaller::purpur();
        let url = installer.download_url("1.20.4", "2176");
        assert!(url.contains("purpur"));
        assert!(url.contains("1.20.4"));
        assert!(url.contains("2176"));
    }

    #[test]
    fn test_paper_installer_type() {
        let installer = PaperInstaller::paper();
        assert_eq!(installer.loader_type(), ModLoaderType::Paper);
    }

    /// Reads the request head before answering: a socket closed with unread
    /// inbound data is reset on Windows, which discards the queued response.
    async fn drain_request(stream: &mut tokio::net::TcpStream) {
        use tokio::io::AsyncReadExt;
        let mut head = [0_u8; 1024];
        let _ = stream.read(&mut head).await;
    }

    /// Serves `bodies` in order, one HTTP response per connection, then hangs up.
    async fn mock_http_server(bodies: Vec<&'static str>) -> u16 {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            for body in bodies {
                let (mut stream, _) = listener.accept().await.unwrap();
                drain_request(&mut stream).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        port
    }

    #[tokio::test]
    async fn fetch_json_recovers_from_transient_bad_payloads() {
        let port = mock_http_server(vec![
            "garbage",
            "still garbage",
            r#"{"id":9,"channel":"STABLE","downloads":{}}"#,
        ])
        .await;
        let client = paper_client().unwrap();
        let url = format!("http://127.0.0.1:{port}/builds");
        let build: PaperBuild = fetch_json(&client, &url).await.unwrap();
        assert_eq!(build.id, 9);
    }

    #[tokio::test]
    async fn fetch_json_gives_up_after_bounded_attempts() {
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_clone = attempts.clone();
        let port = {
            use tokio::io::AsyncWriteExt;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move {
                loop {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    attempts_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    drain_request(&mut stream).await;
                    let response =
                        "HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\ngarbage";
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                }
            });
            port
        };
        let client = paper_client().unwrap();
        let url = format!("http://127.0.0.1:{port}/builds");
        let result: Result<PaperBuild, LoaderInstallError> = fetch_json(&client, &url).await;
        assert!(result.is_err());
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "expected exactly 3 attempts before giving up"
        );
    }

    #[test]
    fn test_paper_synthetic_version_info() {
        let installer = PaperInstaller::paper();
        let info = installer.synthetic_version_info("1.20.4", "496");
        assert_eq!(info.id, "paper-1.20.4-496");
        assert!(info.libraries.is_empty());
    }

    #[tokio::test]
    async fn test_paper_fetch_latest_build() {
        let installer = PaperInstaller::paper();
        let result = installer.fetch_latest_build("1.20.4").await;
        if let Ok(build) = result {
            assert!(!build.is_empty());
            assert!(build.parse::<u64>().is_ok());
        }
        // May fail without network
    }

    #[tokio::test]
    async fn test_paper_install_fails_gracefully_without_network() {
        let installer = PaperInstaller::paper();
        let tmpdir = tempfile::tempdir().unwrap();
        // Use an obviously invalid version to force a quick failure
        let result = installer
            .install("0.0.0-invalid", "99999", tmpdir.path())
            .await;
        assert!(result.is_err());
    }

    /// An explicit nonexistent build must fail without silently selecting latest.
    #[tokio::test]
    async fn test_paper_unknown_numeric_build_rejected() {
        let installer = PaperInstaller::paper();
        let tmpdir = tempfile::tempdir().unwrap();
        // Numeric selection skips latest resolution, but still needs artifact metadata.
        let result = installer.install("1.20.4", "12345", tmpdir.path()).await;
        // Unknown metadata cannot produce a usable artifact.
        assert!(result.is_err());
        let err = result.unwrap_err().to_string().to_lowercase();
        assert!(
            err.contains("download") || err.contains("network") || err.contains("http"),
            "expected download error, got: {err}"
        );
    }
}
