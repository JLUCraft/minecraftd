//! Mojang runtime download planning with host-provided HTTP configuration.
use crate::{
    download::request::DownloadParam,
    resource::{
        ResourceError, ResourceType, SourceType, get_download_api, http::ResourceTransport,
    },
};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Component, Path},
};

fn platform(os: &str, arch: &str) -> Result<&'static str, ResourceError> {
    match (os, arch) {
        ("windows", "aarch64") => Ok("windows-arm64"),
        ("windows", "x86_64") => Ok("windows-x64"),
        ("windows", "x86") => Ok("windows-x86"),
        ("macos", "aarch64") => Ok("mac-os-arm64"),
        ("macos", "x86_64") => Ok("mac-os"),
        ("linux", "x86_64") => Ok("linux"),
        ("linux", "x86") => Ok("linux-i386"),
        _ => Err(ResourceError::Parse(format!(
            "unsupported Java platform: {os}/{arch}"
        ))),
    }
}

pub async fn download_plan<C: ResourceTransport>(
    client: &C,
    sources: &[SourceType],
    os: &str,
    arch: &str,
    version: &str,
    directory: &Path,
) -> Result<Vec<DownloadParam>, ResourceError> {
    let platform = platform(os, arch)?;
    let runtime = match version {
        "8" => "jre-legacy",
        "16" => "java-runtime-alpha",
        "17" => "java-runtime-gamma",
        "21" => "java-runtime-delta",
        "25" => "java-runtime-epsilon",
        _ => {
            return Err(ResourceError::Parse(format!(
                "unsupported Mojang Java version: {version}"
            )));
        }
    };
    let mut last_error = ResourceError::Network("no Mojang Java source configured".into());
    for source in sources {
        let url = get_download_api(*source, ResourceType::MojangJava)?;
        match client
            .request_json(url.as_str(), &HashMap::new(), None, None)
            .await
        {
            Ok(index) => {
                let url = index[platform][runtime][0]["manifest"]["url"]
                    .as_str()
                    .ok_or_else(|| {
                        ResourceError::Parse(format!(
                            "runtime {runtime} unavailable for {platform}"
                        ))
                    })?;
                let manifest = client
                    .request_json(url, &HashMap::new(), None, None)
                    .await?;
                return files(&manifest, directory);
            }
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

fn files(manifest: &Value, directory: &Path) -> Result<Vec<DownloadParam>, ResourceError> {
    let entries = manifest["files"]
        .as_object()
        .ok_or_else(|| ResourceError::Parse("invalid Java runtime files".into()))?;
    let mut downloads = Vec::new();
    for (path, info) in entries {
        if path.contains('\\')
            || path.contains(':')
            || Path::new(path)
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(ResourceError::Parse(format!(
                "unsafe Java runtime path: {path}"
            )));
        }
        if info["type"] != "file" {
            continue;
        }
        let raw = &info["downloads"]["raw"];
        let url = raw["url"]
            .as_str()
            .ok_or_else(|| ResourceError::Parse(format!("missing Java download URL: {path}")))?;
        let sha1 = raw["sha1"]
            .as_str()
            .ok_or_else(|| ResourceError::Parse(format!("missing Java download hash: {path}")))?;
        downloads.push(DownloadParam {
            src: url.parse()?,
            dest: directory.join(path),
            filename: None,
            sha1: Some(sha1.into()),
        });
    }
    Ok(downloads)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_wrong_architecture_and_escaping_paths() {
        assert!(platform("linux", "aarch64").is_err());
        let manifest = serde_json::json!({"files": {"../java": {"type": "file"}}});
        assert!(files(&manifest, Path::new("runtime")).is_err());
        assert!(
            files(
                &serde_json::json!({"files": {"bin/java": {"type":"file"}}}),
                Path::new("runtime")
            )
            .is_err()
        );
    }
}
