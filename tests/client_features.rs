//! Desktop-client behavior is verified here, independently of Tauri.
use minecraftd::minecraft::{java::discovery, loaders::ModLoaderType, metadata, servers, world};
use minecraftd::modpack::{
    ModpackManifest,
    export::{ExportFormat, ExportModpackOptions},
};
use minecraftd::resource::{ResourceError, http::ResourceTransport};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Mutex;

type RecordedRequest = (String, HashMap<String, String>, Option<String>);
#[derive(Default)]
struct RecordingTransport {
    requests: Mutex<Vec<RecordedRequest>>,
}
#[async_trait::async_trait]
impl ResourceTransport for RecordingTransport {
    async fn request_json(
        &self,
        url: &str,
        query: &HashMap<String, String>,
        _body: Option<&Value>,
        key: Option<&str>,
    ) -> Result<Value, ResourceError> {
        self.requests
            .lock()
            .unwrap()
            .push((url.into(), query.clone(), key.map(str::to_owned)));
        if url.contains("curseforge") {
            Ok(
                json!({"data": [], "pagination": {"index":0,"pageSize":50,"resultCount":0,"totalCount":0}}),
            )
        } else {
            Ok(json!([]))
        }
    }
}

#[tokio::test]
async fn configured_transport_receives_escaped_filters_and_api_key() {
    let transport = RecordingTransport::default();
    let client = minecraftd::resource::modrinth::ModrinthClient::with_client(&transport);
    client
        .get_project_versions("example", Some(&["fab\"ric"]), Some(&["1.21"]))
        .await
        .unwrap();
    let client = minecraftd::resource::curseforge::CurseForgeClient::with_client(
        &transport,
        "test-key".into(),
    );
    client
        .get_mod_files_page(
            123,
            &minecraftd::resource::curseforge::FilesQuery {
                game_version: Some("1.21".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let calls = transport.requests.lock().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&calls[0].1["loaders"]).unwrap(),
        json!(["fab\"ric"])
    );
    assert_eq!(calls[1].2.as_deref(), Some("test-key"));
    assert_eq!(calls[1].1["gameVersion"], "1.21");
}

fn export_options(format: ExportFormat) -> ExportModpackOptions {
    ExportModpackOptions {
        format,
        name: "Test pack".into(),
        version: "v1".into(),
        author: None,
        description: Some("Offline export".into()),
        min_memory: Some(1024),
        no_create_remote_files: Some(true),
        skip_curseforge_remote_files: Some(true),
    }
}

#[tokio::test]
async fn offline_modpack_export_retains_files_and_loader_and_roundtrips() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("example.jar.disabled");
    std::fs::write(&file, b"local mod").unwrap();
    let selected = vec![("mods/example.jar.disabled".into(), file)];
    let transport = RecordingTransport::default();
    let bundle = minecraftd::modpack::modrinth::build_modrinth_export_bundle(
        &transport,
        None,
        "1.21",
        ModLoaderType::Quilt,
        "0.26.0",
        &export_options(ExportFormat::Modrinth),
        &selected,
    )
    .await
    .unwrap();
    assert!(transport.requests.lock().unwrap().is_empty());
    assert_eq!(bundle.overrides_files, selected);
    let manifest: minecraftd::modpack::modrinth::ModrinthManifest =
        serde_json::from_str(&bundle.extra_files[0].1).unwrap();
    assert_eq!(manifest.client_version().unwrap(), "1.21");
    assert_eq!(
        manifest.mod_loader().unwrap(),
        Some((ModLoaderType::Quilt, "0.26.0".into()))
    );
    let bundle = minecraftd::modpack::multimc::build_multimc_export_bundle(
        "1.21",
        ModLoaderType::Quilt,
        "0.26.0",
        &export_options(ExportFormat::MultiMC),
        &selected,
    )
    .unwrap();
    let manifest: minecraftd::modpack::multimc::MultiMcManifest =
        serde_json::from_str(&bundle.extra_files[0].1).unwrap();
    assert_eq!(
        manifest.mod_loader().unwrap(),
        Some((ModLoaderType::Quilt, "0.26.0".into()))
    );
    assert!(bundle.extra_files[1].1.contains("MinMemAlloc=1024"));
}

#[tokio::test]
async fn server_list_roundtrip_and_corrupt_data_are_distinct() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(servers::SERVERS_DAT_FILENAME);
    assert!(
        servers::load_servers_info_from_nbt(&path)
            .await
            .unwrap()
            .is_empty()
    );
    let server = servers::GameServerInfo {
        ip: "localhost:25565".into(),
        name: "联机服务器".into(),
        hidden: true,
        icon_src: "icon".into(),
        ..Default::default()
    };
    servers::save_servers_to_nbt(&path, std::slice::from_ref(&server))
        .await
        .unwrap();
    assert_eq!(
        servers::load_servers_info_from_nbt(&path).await.unwrap(),
        vec![server]
    );
    std::fs::write(&path, b"broken").unwrap();
    assert!(matches!(
        servers::load_servers_info_from_nbt(&path).await,
        Err(servers::ServerListError::Nbt(_))
    ));
}

#[tokio::test]
async fn reads_real_nbt_integer_types_in_world_metadata() {
    let root = tempfile::tempdir().unwrap();
    let mut data = quartz_nbt::NbtCompound::new();
    data.insert("LevelName", "World");
    data.insert("LastPlayed", 123_000_i64);
    data.insert("GameType", 1_i32);
    data.insert("Difficulty", 3_i8);
    let mut compound = quartz_nbt::NbtCompound::new();
    compound.insert("Data", data);
    let mut bytes = Vec::new();
    quartz_nbt::io::write_nbt(
        &mut bytes,
        None,
        &compound,
        quartz_nbt::io::Flavor::GzCompressed,
    )
    .unwrap();
    std::fs::write(root.path().join("level.dat"), bytes).unwrap();
    let info = world::load_world_info_from_dir(root.path(), true)
        .await
        .unwrap();
    assert_eq!(info.gamemode, "creative");
    assert_eq!(info.difficulty.as_deref(), Some("hard"));
    assert_eq!(info.last_played_at, 123);
}

#[test]
fn java_release_file_and_early_access_versions() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("bin")).unwrap();
    std::fs::write(
        root.path().join("release"),
        "JAVA_VERSION=\"25-ea\"\nIMPLEMENTOR=\"Example Vendor\"\n",
    )
    .unwrap();
    let java = root.path().join("bin/java");
    let runtime = discovery::inspect(java.to_str().unwrap(), true).unwrap();
    assert_eq!(runtime.major_version, 25);
    assert_eq!(runtime.vendor, "Example Vendor");
    assert!(runtime.is_lts && runtime.is_user_added);
    assert_eq!(discovery::parse_java_major_version("1.8.0_472"), (8, true));
}

#[test]
fn toggling_mods_is_idempotent_and_does_not_overwrite() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("mod.jar");
    std::fs::write(&file, b"mod").unwrap();
    let disabled = metadata::set_enabled(&file, false).unwrap();
    assert_eq!(metadata::set_enabled(&disabled, false).unwrap(), disabled);
    std::fs::write(&file, b"other").unwrap();
    assert!(metadata::set_enabled(&disabled, true).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), b"other");
    std::fs::remove_file(&file).unwrap();
    assert_eq!(metadata::set_enabled(&disabled, true).unwrap(), file);
}

#[test]
fn embedded_native_catalog_and_mirror_origin_checks() {
    assert!(
        !minecraftd::minecraft::natives::native_replacements()
            .unwrap()
            .is_empty()
    );
}
