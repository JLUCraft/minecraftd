//! Integration tests for `modpack`, `resource`, and addon download behaviors.
//!
//! Every test calls a public library API. No manual struct construction that
//! bypasses `from_archive`, `detect_format`, `download_tasks`, etc.

use minecraftd::minecraft::loader::plugin::{AddonInstaller, AddonType};
use minecraftd::modpack::{
    ModpackError, ModpackFormat, ModpackManifest, curseforge::CurseForgeManifest, detect_format,
    extract_overrides,
};
use minecraftd::resource::curseforge::{CurseForgeClient, SearchModsQuery};
use minecraftd::resource::modrinth::{ModrinthClient, SearchQuery};
use std::io::Write;
use std::path::PathBuf;

// ============================================================================
// Helpers
// ============================================================================

fn skip_if_offline() -> bool {
    std::net::ToSocketAddrs::to_socket_addrs(&("api.modrinth.com", 443)).is_err()
}

fn make_zip(path: &std::path::Path, entries: &[(&str, &[u8])]) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in entries {
        zip.start_file(*name, opts).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}

fn zip_path(tmpdir: &tempfile::TempDir, name: &str) -> PathBuf {
    tmpdir.path().join(name)
}

// ============================================================================
// `detect_format` + `from_archive` — no network needed
// ============================================================================

#[tokio::test]
async fn test_detect_and_parse_curseforge_modpack() {
    let tmpdir = tempfile::tempdir().unwrap();
    let zip = zip_path(&tmpdir, "curseforge.zip");

    let manifest_json = serde_json::json!({
        "minecraft": {
            "version": "1.20.1",
            "modLoaders": [{ "id": "forge-47.2.0", "primary": true }]
        },
        "manifestType": "minecraftModpack",
        "manifestVersion": 1,
        "name": "Test CF Pack",
        "version": "1.0.0",
        "author": "test",
        "files": [{
            "projectID": 238222,
            "fileID": 5704646,
            "required": true
        }],
        "overrides": "overrides"
    });
    make_zip(
        &zip,
        &[("manifest.json", manifest_json.to_string().as_bytes())],
    );

    // detect_format
    let format = detect_format(&zip).expect("should detect format");
    assert_eq!(format, ModpackFormat::CurseForge);

    // from_archive
    let manifest = CurseForgeManifest::from_archive(&zip).expect("should parse manifest");
    assert_eq!(manifest.name, "Test CF Pack");
    assert_eq!(manifest.client_version().unwrap(), "1.20.1");

    let (loader_type, loader_ver) = manifest.mod_loader().unwrap().expect("should have loader");
    assert_eq!(
        loader_type,
        minecraftd::minecraft::loaders::ModLoaderType::Forge
    );
    assert_eq!(loader_ver, "47.2.0");

    assert_eq!(manifest.overrides_path(), "overrides");
    assert_eq!(manifest.files.len(), 1);
    assert_eq!(manifest.files[0].project_id, 238222);
}

#[tokio::test]
async fn test_detect_and_parse_modrinth_modpack() {
    let tmpdir = tempfile::tempdir().unwrap();
    let zip = zip_path(&tmpdir, "modrinth.zip");

    let index_json = serde_json::json!({
        "formatVersion": 1,
        "game": "minecraft",
        "versionId": "1.0.0",
        "name": "Test MR Pack",
        "summary": "a test pack",
        "files": [{
            "path": "mods/fabric-api.jar",
            "hashes": { "sha1": "abc123", "sha512": "def456" },
            "env": { "client": "required", "server": "required" },
            "downloads": ["https://example.com/mod.jar"],
            "fileSize": 1024
        }],
        "dependencies": {
            "minecraft": "1.20.4",
            "fabric-loader": "0.15.6"
        }
    });
    make_zip(
        &zip,
        &[("modrinth.index.json", index_json.to_string().as_bytes())],
    );

    // detect_format
    let format = detect_format(&zip).expect("should detect format");
    assert_eq!(format, ModpackFormat::Modrinth);

    // from_archive
    let manifest = minecraftd::modpack::modrinth::ModrinthManifest::from_archive(&zip)
        .expect("should parse manifest");
    assert_eq!(manifest.name, "Test MR Pack");
    assert_eq!(manifest.client_version().unwrap(), "1.20.4");

    let (loader_type, loader_ver) = manifest.mod_loader().unwrap().expect("should have loader");
    assert_eq!(
        loader_type,
        minecraftd::minecraft::loaders::ModLoaderType::Fabric
    );
    assert_eq!(loader_ver, "0.15.6");

    assert_eq!(manifest.overrides_path(), "overrides");
    assert_eq!(manifest.files.len(), 1);
    assert_eq!(manifest.files[0].path, "mods/fabric-api.jar");
}

#[tokio::test]
async fn test_detect_multimc_format() {
    let tmpdir = tempfile::tempdir().unwrap();
    let zip = zip_path(&tmpdir, "multimc.zip");
    make_zip(
        &zip,
        &[
            ("instance.cfg", b"InstanceType=OneSix"),
            ("mmc-pack.json", b"{}"),
        ],
    );

    let format = detect_format(&zip).unwrap();
    assert_eq!(format, ModpackFormat::MultiMc);
}

// ============================================================================
// Error edge cases — invalid zips, missing manifests, bad JSON
// ============================================================================

#[tokio::test]
async fn test_detect_format_unknown_zip() {
    let tmpdir = tempfile::tempdir().unwrap();
    let zip = zip_path(&tmpdir, "unknown.zip");
    make_zip(&zip, &[("readme.txt", b"hello")]);

    let result = detect_format(&zip);
    assert!(matches!(result, Err(ModpackError::UnknownFormat)));
}

#[tokio::test]
async fn test_from_archive_missing_manifest() {
    let tmpdir = tempfile::tempdir().unwrap();
    let zip = zip_path(&tmpdir, "no-manifest.zip");
    make_zip(&zip, &[("some_file.txt", b"hello")]);

    assert!(CurseForgeManifest::from_archive(&zip).is_err());
    assert!(minecraftd::modpack::modrinth::ModrinthManifest::from_archive(&zip).is_err());
}

#[tokio::test]
async fn test_from_archive_invalid_json() {
    let tmpdir = tempfile::tempdir().unwrap();

    // CurseForge: garbage JSON
    let zip = zip_path(&tmpdir, "bad-cf.zip");
    make_zip(&zip, &[("manifest.json", b"not valid json {{{")]);
    assert!(CurseForgeManifest::from_archive(&zip).is_err());

    // Modrinth: garbage JSON
    let zip = zip_path(&tmpdir, "bad-mr.zip");
    make_zip(&zip, &[("modrinth.index.json", b"also not json {")]);
    assert!(minecraftd::modpack::modrinth::ModrinthManifest::from_archive(&zip).is_err());

    // CurseForge: empty object (missing required fields)
    let zip = zip_path(&tmpdir, "empty-cf.zip");
    make_zip(&zip, &[("manifest.json", b"{}")]);
    assert!(
        CurseForgeManifest::from_archive(&zip).is_err(),
        "empty JSON should fail deserialization"
    );
}

// ============================================================================
// `extract_overrides` — extract files from zip overrides folder
// ============================================================================

#[tokio::test]
async fn test_extract_overrides_from_zip() {
    let tmpdir = tempfile::tempdir().unwrap();
    let zip = zip_path(&tmpdir, "with-overrides.zip");
    let instance_path = tmpdir.path().join("instance");

    let manifest_json = serde_json::json!({
        "minecraft": { "version": "1.20.1", "modLoaders": [] },
        "manifestType": "minecraftModpack",
        "manifestVersion": 1,
        "name": "Override Test",
        "version": "1.0.0",
        "author": "test",
        "files": [],
        "overrides": "overrides"
    });

    let file = std::fs::File::create(&zip).unwrap();
    let mut zw = zip::ZipWriter::new(file);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    zw.start_file("manifest.json", opts).unwrap();
    zw.write_all(manifest_json.to_string().as_bytes()).unwrap();
    zw.start_file("overrides/config/test.cfg", opts).unwrap();
    zw.write_all(b"key=value\n").unwrap();
    zw.start_file("overrides/options.txt", opts).unwrap();
    zw.write_all(b"fov:70\n").unwrap();
    zw.add_directory("overrides/empty_dir/", opts).unwrap();
    zw.finish().unwrap();

    extract_overrides(&zip, &instance_path, "overrides").expect("should extract overrides");

    let cfg = instance_path.join("config").join("test.cfg");
    assert!(cfg.exists(), "overrides config file should exist");
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "key=value\n");

    let opts_file = instance_path.join("options.txt");
    assert!(opts_file.exists(), "overrides options.txt should exist");
    assert_eq!(std::fs::read_to_string(&opts_file).unwrap(), "fov:70\n");
}

// ============================================================================
// Export → import round-trip via `generate_manifest` → `from_archive`
// ============================================================================

#[tokio::test]
async fn test_curseforge_export_then_import() {
    use minecraftd::minecraft::loaders::ModLoaderType;
    use minecraftd::modpack::curseforge::{CurseForgeExportOptions, generate_manifest};

    let options = CurseForgeExportOptions {
        name: "ExportTest".into(),
        version: "3.0.0".into(),
        author: "tester".into(),
        overrides: "overrides".into(),
    };

    let manifest = generate_manifest(
        &options,
        "1.20.1",
        Some((ModLoaderType::Forge, "47.2.0".into())),
        &[],
    )
    .unwrap();

    assert_eq!(manifest.name, "ExportTest");
    assert_eq!(manifest.minecraft.version, "1.20.1");

    // Serialize → zip → from_archive
    let tmpdir = tempfile::tempdir().unwrap();
    let zip = zip_path(&tmpdir, "export-test.zip");
    let json_str = serde_json::to_string(&manifest).unwrap();
    make_zip(&zip, &[("manifest.json", json_str.as_bytes())]);

    let parsed = CurseForgeManifest::from_archive(&zip).unwrap();

    assert_eq!(parsed.name, "ExportTest");
    assert_eq!(parsed.client_version().unwrap(), "1.20.1");

    let (loader, ver) = parsed.mod_loader().unwrap().expect("should have loader");
    assert_eq!(loader, ModLoaderType::Forge);
    assert_eq!(ver, "47.2.0");
    assert_eq!(parsed.overrides_path(), "overrides");
}

#[tokio::test]
async fn test_modrinth_export_then_import() {
    use minecraftd::minecraft::loaders::ModLoaderType;
    use minecraftd::modpack::modrinth::{ModrinthExportOptions, generate_manifest};

    let tmpdir_inner = tempfile::tempdir().unwrap();
    let options = ModrinthExportOptions {
        version_id: "1.0.0".into(),
        name: "MR Export Test".into(),
        summary: Some("test export".into()),
    };

    // No files → no Modrinth API calls for version resolution
    let manifest = generate_manifest(
        &options,
        tmpdir_inner.path(),
        "1.21.0",
        Some((ModLoaderType::Fabric, "0.16.0".into())),
        &[],
    )
    .await
    .unwrap();

    assert_eq!(manifest.name, "MR Export Test");
    assert_eq!(manifest.dependencies.get("minecraft").unwrap(), "1.21.0");

    // Serialize → zip → from_archive
    let tmpdir = tempfile::tempdir().unwrap();
    let zip = zip_path(&tmpdir, "mr-export-test.zip");
    let json_str = serde_json::to_string(&manifest).unwrap();
    make_zip(&zip, &[("modrinth.index.json", json_str.as_bytes())]);

    let parsed = minecraftd::modpack::modrinth::ModrinthManifest::from_archive(&zip).unwrap();

    assert_eq!(parsed.name, "MR Export Test");
    assert_eq!(parsed.client_version().unwrap(), "1.21.0");

    let (loader, ver) = parsed.mod_loader().unwrap().expect("should have loader");
    assert_eq!(loader, ModLoaderType::Fabric);
    assert_eq!(ver, "0.16.0");
}

// ============================================================================
// Modpack download tasks — `from_archive` → `download_tasks()` (network)
// ============================================================================

/// CurseForge modpack: parse zip → download_tasks → execute.
/// Requires `MINECRAFTD_CURSEFORGE_API_KEY` env var.
#[tokio::test]
async fn test_curseforge_modpack_download_tasks() {
    if skip_if_offline() {
        eprintln!("SKIP: no network connectivity");
        return;
    }
    let api_key = std::env::var("MINECRAFTD_CURSEFORGE_API_KEY");
    if api_key.is_err() {
        eprintln!("SKIP: MINECRAFTD_CURSEFORGE_API_KEY not set");
        return;
    }

    let tmpdir = tempfile::tempdir().unwrap();
    let zip = zip_path(&tmpdir, "cf-download.zip");
    let instance_path = tmpdir.path().join("instance");

    // JEI for 1.20.1 Forge (project 238222, file 5704646)
    let manifest_json = serde_json::json!({
        "minecraft": {
            "version": "1.20.1",
            "modLoaders": [{ "id": "forge-47.2.0", "primary": true }]
        },
        "manifestType": "minecraftModpack",
        "manifestVersion": 1,
        "name": "CF Download Test",
        "version": "1.0.0",
        "author": "test",
        "files": [
            { "projectID": 238222, "fileID": 5704646, "required": true }
        ],
        "overrides": "overrides"
    });
    make_zip(
        &zip,
        &[("manifest.json", manifest_json.to_string().as_bytes())],
    );

    let manifest = CurseForgeManifest::from_archive(&zip).expect("should parse");
    let tasks = manifest.download_tasks(&instance_path).await;

    match tasks {
        Ok(task_list) => {
            assert!(!task_list.is_empty(), "should generate download tasks");
            eprintln!("{} download task(s)", task_list.len());

            if let Some(task) = task_list.first() {
                eprintln!(
                    "Downloading: {} -> {}",
                    task.url,
                    task.destination.display()
                );
                match task.execute().await {
                    Ok(()) => {
                        assert!(task.destination.exists());
                        let meta = tokio::fs::metadata(&task.destination).await.unwrap();
                        assert!(meta.len() > 1000, "mod JAR should have content");
                        eprintln!("Downloaded {} bytes", meta.len());
                    }
                    Err(e) => eprintln!("Download failed (network): {}", e),
                }
            }
        }
        Err(e) => eprintln!("Task generation failed (network/api): {}", e),
    }
}

/// Modrinth modpack: parse zip → download_tasks → execute.
#[tokio::test]
async fn test_modrinth_modpack_download_tasks() {
    if skip_if_offline() {
        eprintln!("SKIP: no network connectivity");
        return;
    }

    let tmpdir = tempfile::tempdir().unwrap();
    let zip = zip_path(&tmpdir, "mr-download.zip");
    let instance_path = tmpdir.path().join("instance");

    // Fabric API from Modrinth CDN — no API key needed
    let index_json = serde_json::json!({
        "formatVersion": 1,
        "game": "minecraft",
        "versionId": "1.0.0",
        "name": "MR Download Test",
        "files": [{
            "path": "mods/fabric-api.jar",
            "hashes": { "sha1": "abc", "sha512": "def" },
            "downloads": [
                "https://cdn.modrinth.com/data/P7dR8mSH/versions/fabric-api-0.91.3%2B1.20.4.jar"
            ],
            "fileSize": 0
        }],
        "dependencies": { "minecraft": "1.20.4", "fabric-loader": "0.15.6" }
    });
    make_zip(
        &zip,
        &[("modrinth.index.json", index_json.to_string().as_bytes())],
    );

    let manifest =
        minecraftd::modpack::modrinth::ModrinthManifest::from_archive(&zip).expect("should parse");
    let tasks = manifest
        .download_tasks(&instance_path)
        .await
        .expect("should generate download tasks");
    assert!(!tasks.is_empty(), "should generate download tasks");

    if let Some(task) = tasks.first() {
        eprintln!(
            "Downloading: {} -> {}",
            task.url,
            task.destination.display()
        );
        match task.execute().await {
            Ok(()) => {
                assert!(task.destination.exists());
                let meta = tokio::fs::metadata(&task.destination).await.unwrap();
                assert!(meta.len() > 10000, "mod JAR should have content");
                eprintln!("Downloaded {} bytes", meta.len());
            }
            Err(e) => eprintln!("Download failed (network): {}", e),
        }
    }
}

// ============================================================================
// AddonInstaller — plugin + mod from URL
// ============================================================================

#[tokio::test]
async fn test_plugin_install_from_url() {
    if skip_if_offline() {
        eprintln!("SKIP: no network connectivity");
        return;
    }

    let tmpdir = tempfile::tempdir().unwrap();
    let game_dir = tmpdir.path().join("server");

    let installer = AddonInstaller::new();
    let result = installer
        .install_from_url(
            AddonType::Plugin,
            "https://cdn.modrinth.com/data/P1OZGk5p/versions/xGQhobcA/ViaVersion-4.10.2.jar",
            &game_dir,
            Some("ViaVersion.jar"),
        )
        .await;

    match result {
        Ok(path) => {
            assert!(path.exists(), "plugin JAR should exist");
            assert!(path.to_string_lossy().contains("plugins"));
            let meta = tokio::fs::metadata(&path).await.unwrap();
            assert!(meta.len() > 1000);
            eprintln!("Plugin downloaded: {} bytes", meta.len());
        }
        Err(e) => {
            let msg = e.to_string().to_lowercase();
            if msg.contains("http") || msg.contains("download") || msg.contains("network") {
                eprintln!("SKIP: plugin download failed (network): {}", e);
            } else {
                panic!("plugin download failed: {}", e);
            }
        }
    }
}

#[tokio::test]
async fn test_mod_install_from_url() {
    if skip_if_offline() {
        eprintln!("SKIP: no network connectivity");
        return;
    }

    let tmpdir = tempfile::tempdir().unwrap();
    let game_dir = tmpdir.path().join("client");

    let installer = AddonInstaller::new();
    let result = installer
        .install_from_url(
            AddonType::Mod,
            "https://cdn.modrinth.com/data/P7dR8mSH/versions/fabric-api-0.91.3%2B1.20.4.jar",
            &game_dir,
            Some("fabric-api.jar"),
        )
        .await;

    match result {
        Ok(path) => {
            assert!(path.exists(), "mod JAR should exist");
            assert!(path.to_string_lossy().contains("mods"));
            let meta = tokio::fs::metadata(&path).await.unwrap();
            assert!(meta.len() > 10000);
            eprintln!("Mod downloaded: {} bytes", meta.len());
        }
        Err(e) => {
            let msg = e.to_string().to_lowercase();
            if msg.contains("http") || msg.contains("download") || msg.contains("network") {
                eprintln!("SKIP: mod download failed (network): {}", e);
            } else {
                panic!("mod download failed: {}", e);
            }
        }
    }
}

// ============================================================================
// API search — `ModrinthClient::search_projects` + `CurseForgeClient::search_mods`
// ============================================================================

#[tokio::test]
async fn test_modrinth_search_resourcepacks() {
    if skip_if_offline() {
        eprintln!("SKIP: no network connectivity");
        return;
    }

    let client = ModrinthClient::new();
    let result = client
        .search_projects(&SearchQuery {
            query: "Faithful".into(),
            project_type: "resourcepack".into(),
            limit: 5,
            ..Default::default()
        })
        .await;

    match result {
        Ok(hits) => {
            eprintln!(
                "Modrinth resource pack search: {} hits / {} total",
                hits.hits.len(),
                hits.total_hits
            );
            assert!(!hits.hits.is_empty());
            for hit in &hits.hits {
                assert_eq!(hit.project_type, "resourcepack");
            }
        }
        Err(e) => eprintln!("Modrinth search failed (network): {}", e),
    }
}

#[tokio::test]
async fn test_modrinth_search_shaderpacks() {
    if skip_if_offline() {
        eprintln!("SKIP: no network connectivity");
        return;
    }

    let client = ModrinthClient::new();
    let result = client
        .search_projects(&SearchQuery {
            query: "".into(),
            project_type: "shader".into(),
            limit: 5,
            ..Default::default()
        })
        .await;

    match result {
        Ok(hits) => {
            eprintln!(
                "Modrinth shader search: {} hits / {} total",
                hits.hits.len(),
                hits.total_hits
            );
            assert!(!hits.hits.is_empty());
            for hit in &hits.hits {
                assert_eq!(hit.project_type, "shader");
            }
        }
        Err(e) => eprintln!("Modrinth search failed (network): {}", e),
    }
}

#[tokio::test]
async fn test_curseforge_search_resourcepacks() {
    if skip_if_offline() {
        eprintln!("SKIP: no network connectivity");
        return;
    }
    let api_key = std::env::var("MINECRAFTD_CURSEFORGE_API_KEY");
    if api_key.is_err() {
        eprintln!("SKIP: MINECRAFTD_CURSEFORGE_API_KEY not set");
        return;
    }

    let client = CurseForgeClient::new(api_key.unwrap());
    let query = SearchModsQuery {
        class_id: Some(12), // Resource Packs
        page_size: 5,
        ..SearchModsQuery::new("")
    };

    match client.search_mods(&query).await {
        Ok(result) => {
            eprintln!(
                "CurseForge resource pack search: {} mods / {} total",
                result.data.len(),
                result.pagination.total_count
            );
            assert!(!result.data.is_empty() || result.pagination.total_count > 0);
        }
        Err(e) => eprintln!("CurseForge search failed (network): {}", e),
    }
}

#[tokio::test]
async fn test_curseforge_search_shaderpacks() {
    if skip_if_offline() {
        eprintln!("SKIP: no network connectivity");
        return;
    }
    let api_key = std::env::var("MINECRAFTD_CURSEFORGE_API_KEY");
    if api_key.is_err() {
        eprintln!("SKIP: MINECRAFTD_CURSEFORGE_API_KEY not set");
        return;
    }

    let client = CurseForgeClient::new(api_key.unwrap());
    let query = SearchModsQuery {
        class_id: Some(6552), // Shader Packs
        page_size: 5,
        ..SearchModsQuery::new("")
    };

    match client.search_mods(&query).await {
        Ok(result) => {
            eprintln!(
                "CurseForge shader search: {} mods / {} total",
                result.data.len(),
                result.pagination.total_count
            );
        }
        Err(e) => eprintln!("CurseForge search failed (network): {}", e),
    }
}
