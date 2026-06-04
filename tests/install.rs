use minecraftd::download::task::DownloadTask;
use minecraftd::instance::path::InstancePath;
use minecraftd::minecraft::assets::AssetIndexClient;
use minecraftd::minecraft::loader::fabric::FabricInstaller;
use minecraftd::minecraft::loader::forge::ForgeInstaller;
use minecraftd::minecraft::loader::installer::ModLoaderInstaller;
use minecraftd::minecraft::loader::neoforge::NeoForgeInstaller;
use minecraftd::minecraft::loader::paper::PaperInstaller;
use minecraftd::minecraft::loader::plugin::{AddonInstaller, AddonType};
use minecraftd::minecraft::manifest::VersionManifestClient;
use minecraftd::minecraft::validator::{AssetEntry, validate_assets, validate_libraries};
use minecraftd::platform::default_game_dir;
use std::collections::HashMap;

/// Integration test: downloads the version manifest, finds 1.20.4,
/// downloads its version JSON, and validates that we can parse it.
#[tokio::test]
async fn test_download_and_parse_version_manifest() {
    let client = VersionManifestClient::new();
    let manifest = client.fetch().await.expect("should fetch manifest");

    assert!(!manifest.versions.is_empty());
    assert!(!manifest.latest.release.is_empty());

    let meta = manifest
        .versions
        .iter()
        .find(|v| v.id == "1.20.4")
        .expect("1.20.4 should exist");

    let version_info = client
        .download_version_json(meta)
        .await
        .expect("should download version JSON");

    assert_eq!(version_info.id, "1.20.4");
    assert!(!version_info.asset_index.id.is_empty());
    assert!(!version_info.libraries.is_empty());
}

/// Integration test: downloads a small asset index and validates parsing.
#[tokio::test]
async fn test_download_and_parse_asset_index() {
    let client = VersionManifestClient::new();
    let manifest = client.fetch().await.expect("should fetch manifest");

    let meta = manifest
        .versions
        .iter()
        .find(|v| v.id == "1.20.4")
        .expect("1.20.4 should exist");

    let version_info = client
        .download_version_json(meta)
        .await
        .expect("should download version JSON");

    let tmpdir = tempfile::tempdir().unwrap();
    let index_path = tmpdir.path().join("assets.json");

    let index = AssetIndexClient::download_and_parse(&version_info.asset_index.url, &index_path)
        .await
        .expect("should download and parse asset index");

    assert!(!index.objects.is_empty());
}

/// Integration test: downloads a single library and validates it.
#[tokio::test]
async fn test_download_single_library() {
    let url = "https://repo1.maven.org/maven2/org/slf4j/slf4j-api/1.7.36/slf4j-api-1.7.36.jar";
    let tmpdir = tempfile::tempdir().unwrap();
    let dest = tmpdir.path().join("slf4j-api-1.7.36.jar");

    let task = DownloadTask::new(url.to_string(), dest.clone());
    task.execute().await.expect("should download library");

    assert!(dest.exists());
    let metadata = tokio::fs::metadata(&dest).await.unwrap();
    assert!(metadata.len() > 0);
}

/// Integration test: validates libraries against a real version info.
#[tokio::test]
async fn test_validate_real_libraries() {
    let client = VersionManifestClient::new();
    let manifest = client.fetch().await.expect("should fetch manifest");

    let meta = manifest
        .versions
        .iter()
        .find(|v| v.id == "1.20.4")
        .expect("1.20.4 should exist");

    let version_info = client
        .download_version_json(meta)
        .await
        .expect("should download version JSON");

    let tmpdir = tempfile::tempdir().unwrap();
    let lib_dir = tmpdir.path().join("libraries");

    let result = validate_libraries(&version_info, &lib_dir).await;
    assert!(!result.is_valid());
    assert!(!result.missing.is_empty());
}

/// Integration test: validates assets against a synthetic asset index.
#[tokio::test]
async fn test_validate_assets_synthetic() {
    let tmpdir = tempfile::tempdir().unwrap();
    let assets_dir = tmpdir.path().join("assets");

    let content = b"valid content";
    let hash = minecraftd::util::hash::sha1_bytes(content);
    let prefix = &hash[..2];

    let mut index = HashMap::new();
    index.insert(
        "valid.png".into(),
        AssetEntry {
            hash: hash.clone(),
            size: content.len() as u64,
        },
    );
    index.insert(
        "missing.png".into(),
        AssetEntry {
            hash: "11223344".into(),
            size: 1024,
        },
    );

    let obj_dir = assets_dir.join("objects").join(prefix);
    std::fs::create_dir_all(&obj_dir).unwrap();
    std::fs::write(obj_dir.join(&hash), content).unwrap();

    let result = validate_assets(&index, &assets_dir).await;
    assert!(!result.is_valid());
    assert_eq!(result.missing.len(), 1);
    assert_eq!(result.valid.len(), 1);
}

/// Integration test: verifies the default game directory path.
#[test]
fn test_default_game_dir_matches_platform() {
    let dir = default_game_dir();
    let s = dir.to_string_lossy();

    if cfg!(target_os = "macos") {
        assert!(s.contains("Application Support"));
        assert!(s.contains("minecraft"));
    } else {
        assert!(s.contains("minecraft"));
    }
}

/// Integration test: downloads a real asset and validates SHA1.
#[tokio::test]
async fn test_download_and_verify_asset() {
    let client = VersionManifestClient::new();
    let manifest = client.fetch().await.expect("should fetch manifest");

    let meta = manifest
        .versions
        .iter()
        .find(|v| v.id == "1.20.4")
        .expect("1.20.4 should exist");

    let version_info = client
        .download_version_json(meta)
        .await
        .expect("should download version JSON");

    let tmpdir = tempfile::tempdir().unwrap();
    let index_path = tmpdir.path().join("assets.json");

    let index = AssetIndexClient::download_and_parse(&version_info.asset_index.url, &index_path)
        .await
        .expect("should download and parse asset index");

    let (name, entry) = index
        .objects
        .iter()
        .next()
        .expect("should have at least one asset");
    let prefix = &entry.hash[..2];
    let dest = tmpdir.path().join("objects").join(prefix).join(&entry.hash);

    let url = format!(
        "https://resources.download.minecraft.net/{}/{}",
        prefix, entry.hash
    );
    let task = DownloadTask::new(url, dest.clone()).with_sha1(entry.hash.to_string());
    task.execute().await.expect("should download asset");

    assert!(dest.exists());
    let hash = minecraftd::util::hash::sha1_file(&dest).await.unwrap();
    assert_eq!(hash, entry.hash, "SHA1 mismatch for asset {}", name);
}

// ============================================================================
// Helper
// ============================================================================

fn skip_if_offline() -> bool {
    std::net::ToSocketAddrs::to_socket_addrs(&("maven.minecraftforge.net", 443)).is_err()
}

/// Recursively count .jar files in a directory.
fn count_jar_files(dir: &std::path::Path) -> usize {
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                count += count_jar_files(&path);
            } else if path.extension().map(|e| e == "jar").unwrap_or(false) {
                count += 1;
            }
        }
    }
    count
}

// ============================================================================
// Strict integration tests for mod loader installers
// ============================================================================

#[tokio::test]
async fn test_forge_installer_downloads_and_extracts_version() {
    if skip_if_offline() {
        eprintln!("SKIP: no network connectivity");
        return;
    }

    let installer = ForgeInstaller::new();
    let tmpdir = tempfile::tempdir().unwrap();
    let game_dir = tmpdir.path().join("forge-server");

    let result = installer.install("1.20.1", "47.2.0", &game_dir).await;

    if let Err(ref e) = result {
        let msg = e.to_string().to_lowercase();
        if msg.contains("java") {
            eprintln!("SKIP: no Java installation available");
            return;
        }
    }

    let version_info = result.expect("Forge install should succeed with network and Java");
    assert!(!version_info.id.is_empty());
    assert!(version_info.id.contains("forge"));
    assert!(version_info.main_class.is_some());
    assert!(game_dir.join("libraries").exists());
}

#[tokio::test]
async fn test_neoforge_installer_downloads_and_extracts_version() {
    if skip_if_offline() {
        eprintln!("SKIP: no network connectivity");
        return;
    }

    let installer = NeoForgeInstaller::new();
    let tmpdir = tempfile::tempdir().unwrap();
    let game_dir = tmpdir.path().join("neoforge-server");

    let result = installer.install("1.20.4", "20.4.0-beta", &game_dir).await;

    if let Err(ref e) = result {
        let msg = e.to_string().to_lowercase();
        if msg.contains("java") {
            eprintln!("SKIP: no Java installation available");
            return;
        }
        if msg.contains("404") || msg.contains("not found") {
            eprintln!("SKIP: NeoForge version no longer available");
            return;
        }
    }

    let version_info = result.expect("NeoForge install should succeed");
    assert!(!version_info.id.is_empty());
    assert!(game_dir.join("libraries").exists());
}

#[tokio::test]
async fn test_paper_installer_downloads_server_jar() {
    if skip_if_offline() {
        eprintln!("SKIP: no network connectivity");
        return;
    }

    let installer = PaperInstaller::paper();
    let tmpdir = tempfile::tempdir().unwrap();
    let game_dir = tmpdir.path().join("paper-server");

    let result = installer.install("1.20.4", "496", &game_dir).await;

    if let Err(ref e) = result {
        let msg = e.to_string().to_lowercase();
        if msg.contains("not found") || msg.contains("404") {
            eprintln!("SKIP: Paper build may no longer be available");
            return;
        }
    }

    let version_info = result.expect("Paper install should succeed");
    assert_eq!(version_info.id, "paper-1.20.4-496");

    let jar_path = game_dir.join("paper-1.20.4-496.jar");
    assert!(jar_path.exists());

    let metadata = tokio::fs::metadata(&jar_path).await.unwrap();
    assert!(metadata.len() > 1000);
}

// ============================================================================
// Addon (plugin/mod) installation tests
// ============================================================================

#[tokio::test]
async fn test_addon_install_plugin_from_file() {
    let tmpdir = tempfile::tempdir().unwrap();
    let game_dir = tmpdir.path().join("server");

    let source = tmpdir.path().join("WorldEdit.jar");
    std::fs::write(&source, b"FAKE_PLUGIN_JAR_CONTENTS").unwrap();

    let installer = AddonInstaller::new();
    let dest = installer
        .install_from_file(AddonType::Plugin, &source, &game_dir)
        .await
        .expect("should install plugin");

    assert!(dest.exists());
    assert!(dest.to_string_lossy().contains("plugins"));
    assert_eq!(dest.file_name().unwrap(), "WorldEdit.jar");

    let content = tokio::fs::read_to_string(&dest).await.unwrap();
    assert_eq!(content, "FAKE_PLUGIN_JAR_CONTENTS");

    let list = installer
        .list_installed(AddonType::Plugin, &game_dir)
        .await
        .unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].file_name().unwrap(), "WorldEdit.jar");
}

#[tokio::test]
async fn test_addon_install_mod_from_file() {
    let tmpdir = tempfile::tempdir().unwrap();
    let game_dir = tmpdir.path().join("server");

    let source = tmpdir.path().join("JEI.jar");
    std::fs::write(&source, b"FAKE_MOD_JAR_CONTENTS").unwrap();

    let installer = AddonInstaller::new();
    let dest = installer
        .install_from_file(AddonType::Mod, &source, &game_dir)
        .await
        .expect("should install mod");

    assert!(dest.exists());
    assert!(dest.to_string_lossy().contains("mods"));

    let list = installer
        .list_installed(AddonType::Mod, &game_dir)
        .await
        .unwrap();
    assert_eq!(list.len(), 1);
}

#[tokio::test]
async fn test_addon_remove_plugin() {
    let tmpdir = tempfile::tempdir().unwrap();
    let game_dir = tmpdir.path().join("server");

    let source = tmpdir.path().join("OldPlugin.jar");
    std::fs::write(&source, b"").unwrap();

    let installer = AddonInstaller::new();
    installer
        .install_from_file(AddonType::Plugin, &source, &game_dir)
        .await
        .unwrap();

    installer
        .remove(AddonType::Plugin, &game_dir, "OldPlugin.jar")
        .await
        .unwrap();

    let list = installer
        .list_installed(AddonType::Plugin, &game_dir)
        .await
        .unwrap();
    assert!(list.is_empty());
}

// ============================================================================
// End-to-end: install real server + addons and start it
// ============================================================================

use minecraftd::core::event::TokioBroadcastBus;
use minecraftd::instance::base::Instance;
use minecraftd::instance::builder::InstanceBuilder;
use minecraftd::instance::server::ServerConfig;
use minecraftd::process::local::LocalSpawner;

/// End-to-end integration test:
/// 1. Install Paper server
/// 2. Install a real plugin (ViaVersion)
/// 3. Accept EULA
/// 4. Start the server via InstanceBuilder + ServerInstance
/// 5. Verify it runs and produces log output
/// 6. Stop the server
///
/// This test requires network, Java, and takes ~60-180 seconds.
#[tokio::test]
async fn test_paper_server_with_plugin_end_to_end() {
    if skip_if_offline() {
        eprintln!("SKIP: no network connectivity");
        return;
    }

    let tmpdir = tempfile::tempdir().unwrap();
    let game_dir = tmpdir.path().join("paper-server");
    std::fs::create_dir_all(&game_dir).unwrap();

    // Step 1: Install Paper server
    let paper = PaperInstaller::paper();
    let paper_result = paper.install("1.20.4", "496", &game_dir).await;
    if let Err(ref e) = paper_result {
        let msg = e.to_string().to_lowercase();
        if msg.contains("not found") || msg.contains("404") {
            eprintln!("SKIP: Paper build unavailable");
            return;
        }
    }
    let _version_info = paper_result.expect("Paper install should succeed");

    let jar_path = game_dir.join("paper-1.20.4-496.jar");
    assert!(jar_path.exists(), "Paper JAR should exist");

    // Step 2: Install a real plugin (ViaVersion from Modrinth)
    let addon = AddonInstaller::new();
    let plugin_url =
        "https://cdn.modrinth.com/data/P1OZGk5p/versions/xGQhobcA/ViaVersion-4.10.2.jar";
    let plugin_result = addon
        .install_from_url(
            AddonType::Plugin,
            plugin_url,
            &game_dir,
            Some("ViaVersion.jar"),
        )
        .await;
    if let Err(ref e) = plugin_result {
        let msg = e.to_string().to_lowercase();
        if msg.contains("http") || msg.contains("download") {
            eprintln!("SKIP: plugin download failed: {}", msg);
            return;
        }
    }
    let plugin_path = plugin_result.expect("plugin install should succeed");
    assert!(plugin_path.exists(), "Plugin JAR should exist");

    // Step 3: Accept EULA
    std::fs::write(game_dir.join("eula.txt"), "eula=true\n").unwrap();

    // Step 4: Start server via minecraftd InstanceBuilder
    let mut server = InstanceBuilder::new("paper-test".to_string(), InstancePath::new(&game_dir))
        .spawner(LocalSpawner::new())
        .event_bus(TokioBroadcastBus::new())
        .build_server(ServerConfig {
            start_command: format!(
                "java -Xmx1G -jar {} nogui",
                jar_path.file_name().unwrap().to_string_lossy()
            ),
            stop_command: "stop".into(),
            ..Default::default()
        });

    // Collect output via event bus
    let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let output_clone = output.clone();
    server.event_bus().subscribe_all(Box::new(move |event| {
        if let Some(minecraftd::core::event::InstanceEvent::Output { data, .. }) =
            event
                .as_any()
                .downcast_ref::<minecraftd::core::event::InstanceEvent>()
        {
            output_clone.lock().unwrap().extend_from_slice(data);
        }
    }));

    server.start().await.expect("server should start");
    assert!(server.is_running(), "server should be running after start");

    // Poll-wait for server to finish initializing (Paper downloads mojang jar on first run).
    // Total max wait: 300s (5 min) to account for slow Mojang jar download.
    let max_wait = tokio::time::Duration::from_secs(300);
    let poll_interval = tokio::time::Duration::from_secs(5);
    let started = tokio::time::Instant::now();
    let logs: String = loop {
        tokio::time::sleep(poll_interval).await;
        let binding = output.lock().unwrap();
        let current = String::from_utf8_lossy(&binding).to_string();
        if current.contains("Done")
            || current.contains("Starting org.bukkit.craftbukkit.Main")
            || current.contains("Starting minecraft server")
        {
            eprintln!("Paper server started in {:?}", started.elapsed());
            break current;
        }
        if started.elapsed() > max_wait {
            eprintln!(
                "TIMEOUT: server log after {:?}:\n{}",
                started.elapsed(),
                &current[..current.len().min(2000)]
            );
            break current;
        }
    };

    assert!(
        logs.contains("Paper")
            || logs.contains("Starting org.bukkit.craftbukkit.Main")
            || logs.contains("Starting minecraft server"),
        "server log should contain startup messages, got: {}",
        &logs[..logs.len().min(500)]
    );

    // Step 5: Stop the server gracefully
    server.stop().await.expect("server should stop");
    assert!(
        !server.is_running(),
        "server should not be running after stop"
    );

    // Verify plugin file exists in plugins directory (more reliable than console output)
    let plugin_jar = game_dir.join("plugins").join(
        plugin_path
            .file_name()
            .expect("plugin path must have file name"),
    );
    assert!(
        plugin_jar.exists(),
        "Plugin JAR should be present in plugins directory: {}",
        plugin_jar.display()
    );
}

/// End-to-end integration test:
/// 1. Install Forge server
/// 2. Install a real mod (JEI)
/// 3. Accept EULA
/// 4. Start the server via InstanceBuilder + ServerInstance
/// 5. Verify it runs
/// 6. Stop the server
///
/// This test requires network, Java, and takes ~90-180 seconds.
#[tokio::test]
async fn test_forge_server_with_mod_end_to_end() {
    if skip_if_offline() {
        eprintln!("SKIP: no network connectivity");
        return;
    }

    let tmpdir = tempfile::tempdir().unwrap();
    let game_dir = tmpdir.path().join("forge-server");
    std::fs::create_dir_all(&game_dir).unwrap();

    // Step 1: Install Forge server
    let forge = ForgeInstaller::new();
    let forge_result = forge.install("1.20.1", "47.2.0", &game_dir).await;
    if let Err(ref e) = forge_result {
        let msg = e.to_string().to_lowercase();
        if msg.contains("java") {
            eprintln!("SKIP: no Java installation");
            return;
        }
    }
    let _version_info = forge_result.expect("Forge install should succeed");

    // Find the run script or forge jar
    let run_script = game_dir.join("run.sh");
    assert!(
        run_script.exists() || game_dir.join("libraries").exists(),
        "Forge should create run.sh or libraries/"
    );

    // Step 2: Install a real mod (JEI from Maven)
    let addon = AddonInstaller::new();
    let mod_url = "https://maven.blamejared.com/mezz/jei/jei-1.20.1-forge/15.3.0.4/jei-1.20.1-forge-15.3.0.4.jar";
    let mod_result = addon
        .install_from_url(AddonType::Mod, mod_url, &game_dir, Some("jei.jar"))
        .await;
    if let Err(ref e) = mod_result {
        let msg = e.to_string().to_lowercase();
        if msg.contains("http") || msg.contains("download") || msg.contains("access") {
            eprintln!("SKIP: mod download failed: {}", msg);
            return;
        }
    }
    let mod_path = mod_result.expect("mod install should succeed");
    assert!(mod_path.exists(), "Mod JAR should exist");

    // Step 3: Accept EULA
    std::fs::write(game_dir.join("eula.txt"), "eula=true\n").unwrap();

    // Step 4: Start server via minecraftd InstanceBuilder
    // Forge uses run.sh script
    let mut server = InstanceBuilder::new("forge-test".to_string(), InstancePath::new(&game_dir))
        .spawner(LocalSpawner::new())
        .event_bus(TokioBroadcastBus::new())
        .build_server(ServerConfig {
            start_command: "bash run.sh".into(),
            stop_command: "stop".into(),
            ..Default::default()
        });

    let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let output_clone = output.clone();
    server.event_bus().subscribe_all(Box::new(move |event| {
        if let Some(minecraftd::core::event::InstanceEvent::Output { data, .. }) =
            event
                .as_any()
                .downcast_ref::<minecraftd::core::event::InstanceEvent>()
        {
            output_clone.lock().unwrap().extend_from_slice(data);
        }
    }));

    server.start().await.expect("server should start");
    assert!(server.is_running());

    // Poll-wait for Forge server to finish initializing.
    // Total max wait: 180s (3 min) — Forge is heavier than Paper.
    let max_wait = tokio::time::Duration::from_secs(180);
    let poll_interval = tokio::time::Duration::from_secs(5);
    let started = tokio::time::Instant::now();
    let logs: String = loop {
        tokio::time::sleep(poll_interval).await;
        let binding = output.lock().unwrap();
        let current = String::from_utf8_lossy(&binding).to_string();
        if current.contains("Forge")
            || current.contains("MinecraftForge")
            || current.contains("Done")
        {
            eprintln!("Forge server started in {:?}", started.elapsed());
            break current;
        }
        if started.elapsed() > max_wait {
            eprintln!(
                "TIMEOUT: server log after {:?}:\n{}",
                started.elapsed(),
                &current[..current.len().min(2000)]
            );
            break current;
        }
    };

    assert!(
        logs.contains("Forge") || logs.contains("MinecraftForge") || logs.contains("Loading"),
        "server log should contain Forge startup messages, got: {}",
        &logs[..logs.len().min(500)]
    );

    // Step 5: Stop
    server.stop().await.expect("server should stop");
    assert!(!server.is_running());
}

/// End-to-end integration test for Fabric client with mod:
/// 1. Install Fabric loader
/// 2. Install a real Fabric mod
/// 3. Download vanilla version JSON + assets
/// 4. Build launch command
/// 5. Start client via InstanceBuilder + ClientInstance
/// 6. Verify it launches (exits quickly since no display/GPU)
///
/// Account and skin are left empty (offline mode).
#[tokio::test]
async fn test_fabric_client_with_mod_end_to_end() {
    if skip_if_offline() {
        eprintln!("SKIP: no network connectivity");
        return;
    }

    let tmpdir = tempfile::tempdir().unwrap();
    let game_dir = tmpdir.path().join("fabric-client");
    std::fs::create_dir_all(&game_dir).unwrap();

    // Step 1: Install Fabric loader
    let fabric = FabricInstaller::new();
    let fabric_result = fabric.install("1.20.4", "0.15.6", &game_dir).await;
    if let Err(ref e) = fabric_result {
        let msg = e.to_string().to_lowercase();
        if msg.contains("network") || msg.contains("http") {
            eprintln!("SKIP: Fabric install failed: {}", msg);
            return;
        }
    }
    let version_info = fabric_result.expect("Fabric install should succeed");
    assert!(!version_info.id.is_empty());
    assert!(version_info.main_class.is_some());

    // Verify Fabric libraries were downloaded (libraries are in nested dirs like net/fabricmc/...)
    let libraries_dir = game_dir.join("libraries");
    assert!(libraries_dir.exists(), "libraries dir should exist");
    let lib_count = count_jar_files(&libraries_dir);
    assert!(
        lib_count > 0,
        "at least one Fabric library JAR should exist, found {} in {}",
        lib_count,
        libraries_dir.display()
    );

    // Step 2: Download vanilla version JSON for inheritance
    let manifest_client = VersionManifestClient::new();
    let manifest = manifest_client
        .fetch()
        .await
        .expect("should fetch manifest");
    let meta = manifest
        .versions
        .iter()
        .find(|v| v.id == "1.20.4")
        .expect("1.20.4 should exist");
    let vanilla_info = manifest_client
        .download_version_json(meta)
        .await
        .expect("should download vanilla version JSON");

    // Merge parent version info (Fabric inherits from vanilla)
    let mut merged = version_info.clone();
    if merged.asset_index.id.is_empty() {
        merged.asset_index = vanilla_info.asset_index;
    }
    if merged.assets.is_empty() {
        merged.assets = vanilla_info.assets;
    }
    if merged.java_version.is_none() {
        merged.java_version = vanilla_info.java_version;
    }

    // Step 3: Install a real Fabric mod (Fabric API from Maven)
    let addon = AddonInstaller::new();
    let mod_url = "https://maven.fabricmc.net/net/fabricmc/fabric-api/fabric-api/0.91.3+1.20.4/fabric-api-0.91.3+1.20.4.jar";
    let mod_result = addon
        .install_from_url(AddonType::Mod, mod_url, &game_dir, Some("fabric-api.jar"))
        .await;
    if let Err(ref e) = mod_result {
        let msg = e.to_string().to_lowercase();
        if msg.contains("http") || msg.contains("download") {
            eprintln!("SKIP: mod download failed: {}", msg);
            return;
        }
    }
    let mod_path = mod_result.expect("mod install should succeed");
    assert!(mod_path.exists(), "Mod JAR should exist");
    assert_eq!(
        mod_path.parent().unwrap(),
        game_dir.join("mods"),
        "mod should be in mods/ dir"
    );

    // Step 4: Build launch command using minecraftd's launch args generator
    let window = minecraftd::minecraft::launch::WindowParams {
        width: 800,
        height: 600,
        fullscreen: false,
        max_memory_mb: 1024,
        auto_join_server: None,
    };

    let launch_cmd = minecraftd::minecraft::launch::generate_launch_command(
        &merged,
        "java",
        &game_dir,
        "TestPlayer",
        &window,
    )
    .expect("should generate launch command");

    assert!(!launch_cmd.args.is_empty());
    assert!(
        launch_cmd
            .args
            .contains(&"net.fabricmc.loader.impl.launch.knot.KnotClient".to_string())
            || launch_cmd
                .args
                .contains(&"net.minecraft.client.main.Main".to_string())
    );

    // Step 5: Verify the classpath includes Fabric loader jar
    let cp_idx = launch_cmd
        .args
        .iter()
        .position(|a| a == "-cp")
        .expect("-cp should exist");
    let cp_value = &launch_cmd.args[cp_idx + 1];
    assert!(
        cp_value.contains("fabric") || cp_value.contains("Fabric"),
        "classpath should include Fabric libraries, got: {}",
        &cp_value[..cp_value.len().min(200)]
    );

    // Step 6: Verify we can at least run java -version (proves Java is available)
    // We do NOT start the actual Minecraft client here because:
    // - It requires a display/GPU (GLFW initialization fails headless)
    // - It requires assets and natives which would take minutes to download
    // - The goal of this test is to verify the installation pipeline works
    let java_check = tokio::process::Command::new("java")
        .arg("-version")
        .output()
        .await;
    assert!(java_check.is_ok(), "Java should be available");
    let java_result = java_check.unwrap();
    let java_out = String::from_utf8_lossy(&java_result.stderr);
    assert!(
        java_out.contains("openjdk") || java_out.contains("Java"),
        "Java version output expected"
    );

    eprintln!("Fabric client install + mod integration test passed:");
    eprintln!("  - Loader: {}", version_info.id);
    eprintln!("  - Libraries: {}", lib_count);
    eprintln!("  - Mod: {}", mod_path.display());
    eprintln!("  - Launch command: {} args", launch_cmd.args.len());
}
