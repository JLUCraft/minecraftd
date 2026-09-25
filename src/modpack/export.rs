use crate::modpack::ModpackError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModpackFileList {
    pub all: Vec<String>,
    pub unchecked: Vec<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ExportFormat {
    Modrinth,
    MultiMC,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportModpackOptions {
    pub format: ExportFormat,
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub min_memory: Option<u32>,                    // for MultiMC
    pub no_create_remote_files: Option<bool>,       // for Modrinth
    pub skip_curseforge_remote_files: Option<bool>, // for Modrinth
}

#[derive(Debug, Clone)]
pub struct ModpackExportBundle {
    pub overrides_prefix: String,
    pub overrides_files: Vec<(String, PathBuf)>,
    pub extra_files: Vec<(String, String)>,
}

const REGEX_BLACKLIST: &[&str] = &[
    r".*\.log$",
    r".*-natives$",
    r"natives-.*$",
    r"^\._.*",
    // Backup files
    r".*\.dat_old$",
    r".*\.old$",
    // BakaXL
    r".*\.BakaCoreInfo$",
];

const BLACKLIST: &[&str] = &[
    ".DS_Store",
    "desktop.ini",
    "Thumbs.db",
    // Minecraft
    "usernamecache.json",
    "usercache.json",
    "jars",
    "logs",
    "versions",
    "assets",
    "libraries",
    "crash-reports",
    "NVIDIA",
    "AMD",
    "screenshots",
    "natives",
    "native",
    "$native",
    "$natives",
    "server-resource-packs",
    "command_history.txt",
    // Old Minecraft Launcher
    "launcher_profiles.json",
    "launcher.pack.lzma",
    // New Minecraft Launcher
    "launcher_accounts.json",
    "launcher_cef_log.txt",
    "launcher_log.txt",
    "launcher_msa_credentials.bin",
    "launcher_settings.json",
    "launcher_ui_state.json",
    "realms_persistence.json",
    "webcache2",
    "treatment_tags.json",
    // Plain Craft Launcher
    "clientId.txt",
    "PCL.ini",
    // HMCL
    "backup",
    "pack.json",
    "launcher.jar",
    "cache",
    "modpack.cfg",
    "log4j2.xml",
    "hmclversion.cfg",
    // SJMCL
    "install_profile.json",
    "sjmclcfg.json",
    // Curse
    "manifest.json",
    "minecraftinstance.json",
    ".curseclient",
    // Modrinth
    "modrinth.index.json",
    // Fabric/OptiFine
    ".fabric",
    ".mixin.out",
    ".optifine",
    // Downloads and Essential
    "downloads",
    "essential",
    // Mods
    "asm",
    "backups",
    "TCNodeTracker",
    "CustomDISkins",
    "data",
    "CustomSkinLoader/caches",
    // Debug files
    "debug",
    // ReplayMod
    ".replay_cache",
    "replay_recordings",
    "replay_videos",
    // Iris
    "irisUpdateInfo.json",
    // ModernFix
    "modernfix",
    // Mod translations
    "modtranslations",
    // Schematics mod
    "schematics",
    // JourneyMap
    "journeymap/data",
    // Sinytra Connector
    "mods/.connector",
];

// ugly, consider GlobSet if this grows too large
const SUGGESTED_BLACKLIST: &[&str] = &[
    // BetterFonts
    "fonts",
    // Minecraft
    "saves",
    "servers.dat",
    "options.txt",
    // BuildCraft
    "blueprints",
    // OptiFine
    "optionsof.txt",
    // JourneyMap
    "journeymap",
    "optionsshaders.txt",
    // VoxelMods
    "mods/VoxelMods",
];

const SUGGESTED_REGEX_BLACKLIST: &[&str] = &[
    r"^fonts/.*$",
    r"^saves/.*$",
    r"^blueprints/.*$",
    r"^journeymap/.*$",
    r"^mods/VoxelMods/.*$",
];

/// List all files that can be offered to the user in the modpack export tree UI.
pub fn list_files(root: &Path, name: &str) -> Result<ModpackFileList, ModpackError> {
    let patterns = regex::RegexSet::new(REGEX_BLACKLIST)
        .map_err(|error| ModpackError::Parse(error.to_string()))?;
    let suggested = regex::RegexSet::new(SUGGESTED_REGEX_BLACKLIST)
        .map_err(|error| ModpackError::Parse(error.to_string()))?;
    if !root.exists() {
        return Err(ModpackError::Io(format!(
            "instance directory missing: {}",
            root.display()
        )));
    }

    let mut files = Vec::new();
    let mut unchecked_files = Vec::new();

    let walker = WalkDir::new(root).into_iter();
    let iter = walker.filter_entry(|entry| {
        let path = entry.path();

        let rel_path = match path.strip_prefix(root) {
            Ok(rel) => rel,
            Err(_) => return false,
        };
        let rel_str = rel_path.to_string_lossy().replace('\\', "/");

        if BLACKLIST.contains(&rel_str.as_str()) {
            return false;
        }
        if patterns.is_match(rel_str.as_str()) {
            return false;
        }
        true
    });

    for entry in iter {
        let entry = entry.map_err(|error| ModpackError::Io(error.to_string()))?;

        if entry.file_type().is_file() {
            let path = entry.path();
            // Ignore name.jar / name.json
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                && stem == name
            {
                continue;
            }

            if let Ok(rel_path) = path.strip_prefix(root) {
                let rel_str = rel_path.to_string_lossy().replace('\\', "/");

                if SUGGESTED_BLACKLIST.contains(&rel_str.as_str()) || suggested.is_match(&rel_str) {
                    unchecked_files.push(rel_str.clone());
                }

                files.push(rel_str);
            }
        }
    }

    files.sort_unstable();
    unchecked_files.sort_unstable();
    Ok(ModpackFileList {
        all: files,
        unchecked: unchecked_files,
    })
}

pub fn normalize_mod_loader_version(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some((_, version)) = trimmed.split_once("-forge-") {
        return version.to_owned();
    }
    trimmed
        .strip_prefix("neoforge-")
        .unwrap_or(trimmed)
        .to_owned()
}
