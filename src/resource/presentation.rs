//! Shared conversion from repository API schemas to resource listings.
use crate::resource::curseforge::{
    CurseForgeFile as CurseForgeFileInfo, CurseForgeProject, SearchResult as CurseForgeSearchRes,
};
use crate::resource::models::*;
use crate::resource::modrinth::{
    ModrinthFile as ModrinthFileInfo, ModrinthProject, ModrinthSearchRes,
    ModrinthVersion as ModrinthVersionPack,
};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::LazyLock;
pub fn version_pack_sort(a: &OtherResourceVersionPack, b: &OtherResourceVersionPack) -> Ordering {
    fn parse_version(version: &str) -> (Vec<u32>, String) {
        let mut version_numbers = Vec::new();
        let mut suffix = String::new();

        for part in version.split('.') {
            if let Some(dash_pos) = part.find('-') {
                let (num_part, suffix_part) = part.split_at(dash_pos);
                if let Ok(num) = num_part.parse::<u32>() {
                    version_numbers.push(num);
                    suffix = suffix_part.to_string();
                }
                break;
            } else if let Ok(num) = part.parse::<u32>() {
                version_numbers.push(num);
            }
        }

        (version_numbers, suffix)
    }

    fn compare_versions_with_suffix(
        v1: &[u32],
        suffix1: &str,
        v2: &[u32],
        suffix2: &str,
    ) -> Ordering {
        for (a, b) in v1.iter().zip(v2.iter()) {
            match a.cmp(b) {
                Ordering::Equal => continue,
                other => return other,
            }
        }

        match v1.len().cmp(&v2.len()) {
            Ordering::Equal => match (suffix1.is_empty(), suffix2.is_empty()) {
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                _ => suffix1.cmp(suffix2),
            },
            other => other,
        }
    }

    let (version_a, suffix_a) = parse_version(&a.name);
    let (version_b, suffix_b) = parse_version(&b.name);

    compare_versions_with_suffix(&version_a, &suffix_a, &version_b, &suffix_b).reverse()
}

pub fn map_modrinth_file_to_version_pack(
    res: Vec<ModrinthVersionPack>,
    resource_type: &str,
) -> Vec<OtherResourceVersionPack> {
    let mut version_packs = std::collections::HashMap::new();
    let is_modpack = resource_type == "modpack";

    for version in res {
        let game_versions = if version.game_versions.is_empty() {
            vec!["".to_string()]
        } else {
            version.game_versions.clone()
        };

        const ALLOWED_LOADERS: &[&str] = &[
            "forge",
            "fabric",
            "quilt",
            "neoforge",
            "vanilla",
            "iris",
            "canvas",
            "optifine",
            "minecraft",
        ];

        let loaders = if version.loaders.is_empty() {
            vec!["".to_string()]
        } else {
            version
                .loaders
                .iter()
                .filter(|loader| ALLOWED_LOADERS.contains(&loader.as_str()))
                .cloned()
                .collect::<Vec<_>>()
        };

        for game_version in &game_versions {
            if is_modpack {
                let file = version
                    .files
                    .iter()
                    .find(|file| file.primary)
                    .or_else(|| version.files.first());

                if let Some(file) = file {
                    version_packs
                        .entry(game_version.clone())
                        .or_insert_with(|| OtherResourceVersionPack {
                            name: game_version.clone(),
                            items: Vec::new(),
                        })
                        .items
                        .push((&version, file, None).into());
                }
            } else {
                for loader in &loaders {
                    let file_infos = version
                        .files
                        .iter()
                        .map(|file| {
                            (
                                &version,
                                file,
                                crate::resource::modrinth::normalize_loader(loader),
                            )
                                .into()
                        })
                        .collect::<Vec<_>>();

                    version_packs
                        .entry(game_version.clone())
                        .or_insert_with(|| OtherResourceVersionPack {
                            name: game_version.clone(),
                            items: Vec::new(),
                        })
                        .items
                        .extend(file_infos);
                }
            }
        }
    }

    let mut list: Vec<OtherResourceVersionPack> = version_packs.into_values().collect();
    list.sort_by(version_pack_sort);

    list
}

impl From<ModrinthProject> for OtherResourceInfo {
    fn from(project: ModrinthProject) -> Self {
        Self {
            id: project.project_id,
            mcmod_id: 0,
            _type: project.project_type,
            name: project.title,
            slug: project.slug.to_string(),
            description: project.description,
            icon_src: project.icon_url.unwrap_or_default(),
            website_url: format!("https://modrinth.com/mod/{}", project.slug),
            tags: project.categories,
            last_updated: project.date_modified,
            downloads: project.downloads,
            source: OtherResourceSource::Modrinth,
            translated_name: None,
            translated_description: None,
            author: project.author,
        }
    }
}

impl From<(&ModrinthVersionPack, &ModrinthFileInfo, Option<String>)> for OtherResourceFileInfo {
    fn from(
        (version, file, loader): (&ModrinthVersionPack, &ModrinthFileInfo, Option<String>),
    ) -> Self {
        Self {
            resource_id: version.project_id.clone(),
            name: version.name.clone(),
            release_type: version.version_type.clone(),
            downloads: version.downloads,
            file_date: version.date_published.clone(),
            download_url: file.url.clone(),
            sha1: file.hashes.sha1.clone(),
            file_name: file.filename.clone(),
            dependencies: version
                .dependencies
                .iter()
                .map(|d| OtherResourceDependency {
                    resource_id: d.project_id.clone().unwrap_or_default(),
                    relation: d.dependency_type.clone(),
                })
                .collect(),
            loader,
        }
    }
}

impl From<ModrinthSearchRes> for OtherResourceSearchRes {
    fn from(res: ModrinthSearchRes) -> Self {
        let list = res.hits.into_iter().map(OtherResourceInfo::from).collect();

        Self {
            list,
            total: res.total_hits,
            page: res.offset.checked_div(res.limit).unwrap_or_default(),
            page_size: res.limit,
        }
    }
}

fn extract_versions_and_loaders(game_versions: &[String]) -> (Vec<String>, Vec<String>) {
    let mut versions = Vec::new();
    let mut loaders = Vec::new();

    const ALLOWED_LOADERS: &[&str] = &[
        "Forge", "Fabric", "Quilt", "NeoForge", "Vanilla", "Iris", "Canvas", "OptiFine",
    ];

    for v in game_versions {
        if v.starts_with(|c: char| c.is_ascii_digit()) {
            versions.push(v.clone());
        } else if ALLOWED_LOADERS.contains(&v.as_str()) {
            loaders.push(v.clone());
        }
    }

    (versions, loaders)
}

pub fn map_curseforge_file_to_version_pack(
    res: Vec<CurseForgeFileInfo>,
) -> Vec<OtherResourceVersionPack> {
    let mut version_packs = std::collections::HashMap::new();

    for cf_file in res {
        let (versions, loaders) = extract_versions_and_loaders(&cf_file.game_versions);

        let versions = if versions.is_empty() {
            vec!["".to_string()]
        } else {
            versions
        };

        let loaders = if loaders.is_empty() {
            vec![""]
        } else {
            loaders.iter().map(|s| s.as_str()).collect::<Vec<_>>()
        };

        for version in &versions {
            for loader in &loaders {
                let file_info = (
                    &cf_file,
                    if loader.is_empty() {
                        None
                    } else {
                        Some(loader.to_string())
                    },
                )
                    .into();

                version_packs
                    .entry(version.clone())
                    .or_insert_with(|| OtherResourceVersionPack {
                        name: version.clone(),
                        items: Vec::new(),
                    })
                    .items
                    .push(file_info);
            }
        }
    }

    let mut list: Vec<OtherResourceVersionPack> = version_packs.into_values().collect();
    list.sort_by(version_pack_sort);

    list
}

impl From<CurseForgeProject> for OtherResourceInfo {
    fn from(project: CurseForgeProject) -> Self {
        Self {
            id: project.id.to_string(),
            mcmod_id: 0,
            _type: cvt_class_id_to_type(project.class_id.unwrap_or(0)),
            name: project.name,
            slug: project.slug,
            description: project.summary,
            icon_src: project.logo.map_or_else(String::new, |logo| logo.url),
            website_url: project.links.website_url,
            tags: project.categories.iter().map(|c| c.name.clone()).collect(),
            last_updated: project.date_modified,
            downloads: project.download_count,
            source: OtherResourceSource::CurseForge,
            translated_name: None,
            translated_description: None,
            author: project.authors.first().map(|author| author.name.clone()),
        }
    }
}

impl From<(&CurseForgeFileInfo, Option<String>)> for OtherResourceFileInfo {
    fn from((cf_file, loader): (&CurseForgeFileInfo, Option<String>)) -> Self {
        Self {
            resource_id: cf_file.mod_id.to_string(),
            name: cf_file.display_name.clone(),
            release_type: cvt_id_to_release_type(cf_file.release_type),
            downloads: cf_file.download_count,
            file_date: cf_file.file_date.clone(),
            download_url: cf_file.download_url.clone().unwrap_or_else(|| {
                crate::resource::curseforge::fallback_download_url(cf_file.id, &cf_file.file_name)
            }),
            sha1: cf_file
                .hashes
                .iter()
                .find(|h| h.algo == 1)
                .map_or_else(String::new, |h| h.value.clone()),
            file_name: cf_file.file_name.clone(),
            dependencies: cf_file
                .dependencies
                .iter()
                .map(|dep| OtherResourceDependency {
                    resource_id: dep.mod_id.to_string(),
                    relation: cvt_id_to_dependency_type(dep.relation_type),
                })
                .collect(),
            loader,
        }
    }
}

impl From<CurseForgeSearchRes> for OtherResourceSearchRes {
    fn from(res: CurseForgeSearchRes) -> Self {
        let list = res.data.into_iter().map(OtherResourceInfo::from).collect();

        Self {
            list,
            total: res.pagination.total_count,
            page: res
                .pagination
                .index
                .checked_div(res.pagination.page_size)
                .unwrap_or_default(),
            page_size: res.pagination.page_size,
        }
    }
}

static CATEGORY_MAP: LazyLock<HashMap<(String, u32), u32>> = LazyLock::new(|| {
    let mut map = HashMap::new();

    // mods
    map.insert(("Food".to_string(), 6), 436);
    map.insert(("Ores and Resources".to_string(), 6), 408);
    map.insert(("Miscellaneous".to_string(), 6), 425);
    map.insert(("Thermal Expansion".to_string(), 6), 427);
    map.insert(("Cosmetic".to_string(), 6), 424);
    map.insert(("Education".to_string(), 6), 5299);
    map.insert(("Buildcraft".to_string(), 6), 432);
    map.insert(("Processing".to_string(), 6), 413);
    map.insert(("Map and Information".to_string(), 6), 423);
    map.insert(("Tinker's Construct".to_string(), 6), 428);
    map.insert(("Industrial Craft".to_string(), 6), 429);
    map.insert(("Technology".to_string(), 6), 412);
    map.insert(("Farming".to_string(), 6), 416);
    map.insert(("Structures".to_string(), 6), 409);
    map.insert(("Genetics".to_string(), 6), 418);
    map.insert(("Magic".to_string(), 6), 419);
    map.insert(("Addons".to_string(), 6), 426);
    map.insert(("Dimensions".to_string(), 6), 410);
    map.insert(("Mobs".to_string(), 6), 411);
    map.insert(("Armor, Tools, and Weapons".to_string(), 6), 434);
    map.insert(("Server Utility".to_string(), 6), 435);
    map.insert(("Energy, Fluid, and Item Transport".to_string(), 6), 415);
    map.insert(("World Gen".to_string(), 6), 406);
    map.insert(("Player Transport".to_string(), 6), 414);
    map.insert(("Forestry".to_string(), 6), 433);
    map.insert(("Applied Energistics 2".to_string(), 6), 4545);
    map.insert(("Energy".to_string(), 6), 417);
    map.insert(("Adventure and RPG".to_string(), 6), 422);
    map.insert(("Storage".to_string(), 6), 420);
    map.insert(("Biomes".to_string(), 6), 407);
    map.insert(("Redstone".to_string(), 6), 4558);
    map.insert(("Thaumcraft".to_string(), 6), 430);
    map.insert(("Blood Magic".to_string(), 6), 4485);
    map.insert(("API and Library".to_string(), 6), 421);
    map.insert(("Twitch Integration".to_string(), 6), 4671);
    map.insert(("Automation".to_string(), 6), 4843);
    map.insert(("CraftTweaker".to_string(), 6), 4773);
    map.insert(("MCreator".to_string(), 6), 4906);
    map.insert(("KubeJS".to_string(), 6), 5314);
    map.insert(("Utility & QoL".to_string(), 6), 5191);
    map.insert(("Galacticraft".to_string(), 6), 5232);
    map.insert(("Skyblock".to_string(), 6), 6145);
    map.insert(("Create".to_string(), 6), 6484);
    map.insert(("Integrated Dynamics".to_string(), 6), 6954);
    map.insert(("Performance".to_string(), 6), 6814);
    map.insert(("Bug Fixes".to_string(), 6), 6821);
    map.insert(("Twilight Forest".to_string(), 6), 7669);

    // resource packs
    map.insert(("Photo Realistic".to_string(), 12), 400);
    map.insert(("Steampunk".to_string(), 12), 399);
    map.insert(("Traditional".to_string(), 12), 403);
    map.insert(("512x and Higher".to_string(), 12), 398);
    map.insert(("128x".to_string(), 12), 396);
    map.insert(("256x".to_string(), 12), 397);
    map.insert(("64x".to_string(), 12), 395);
    map.insert(("Medieval".to_string(), 12), 402);
    map.insert(("Miscellaneous".to_string(), 12), 405);
    map.insert(("32x".to_string(), 12), 394);
    map.insert(("16x".to_string(), 12), 393);
    map.insert(("Animated".to_string(), 12), 404);
    map.insert(("Modern".to_string(), 12), 401);
    map.insert(("Mod Support".to_string(), 12), 4465);
    map.insert(("Data Packs".to_string(), 12), 5193);
    map.insert(("Font Packs".to_string(), 12), 5244);

    // worlds
    map.insert(("Parkour".to_string(), 17), 251);
    map.insert(("Survival".to_string(), 17), 253);
    map.insert(("Creation".to_string(), 17), 249);
    map.insert(("Game Map".to_string(), 17), 250);
    map.insert(("Adventure".to_string(), 17), 248);
    map.insert(("Modded World".to_string(), 17), 4464);
    map.insert(("Puzzle".to_string(), 17), 252);

    // mod packs
    map.insert(("Adventure and RPG".to_string(), 4471), 4475);
    map.insert(("FTB Official Pack".to_string(), 4471), 4487);
    map.insert(("Small / Light".to_string(), 4471), 4481);
    map.insert(("Quests".to_string(), 4471), 4478);
    map.insert(("Combat / PvP".to_string(), 4471), 4483);
    map.insert(("Tech".to_string(), 4471), 4472);
    map.insert(("Sci-Fi".to_string(), 4471), 4474);
    map.insert(("Multiplayer".to_string(), 4471), 4484);
    map.insert(("Hardcore".to_string(), 4471), 4479);
    map.insert(("Mini Game".to_string(), 4471), 4477);
    map.insert(("Extra Large".to_string(), 4471), 4482);
    map.insert(("Magic".to_string(), 4471), 4473);
    map.insert(("Skyblock".to_string(), 4471), 4736);
    map.insert(("Map Based".to_string(), 4471), 4480);
    map.insert(("Exploration".to_string(), 4471), 4476);
    map.insert(("Vanilla+".to_string(), 4471), 5128);
    map.insert(("Horror".to_string(), 4471), 7418);

    // shader packs
    map.insert(("Vanilla".to_string(), 6552), 6555);
    map.insert(("Fantasy".to_string(), 6552), 6554);
    map.insert(("Realistic".to_string(), 6552), 6553);

    // data packs
    map.insert(("Magic".to_string(), 6945), 6952);
    map.insert(("Miscellaneous".to_string(), 6945), 6947);
    map.insert(("Fantasy".to_string(), 6945), 6949);
    map.insert(("Mod Support".to_string(), 6945), 6946);
    map.insert(("Tech".to_string(), 6945), 6951);
    map.insert(("Library".to_string(), 6945), 6950);
    map.insert(("Utility".to_string(), 6945), 6953);
    map.insert(("Adventure".to_string(), 6945), 6948);
    map
});

pub fn cvt_category_to_id(category: &str, class_id: u32) -> u32 {
    CATEGORY_MAP
        .get(&(category.to_owned(), class_id))
        .copied()
        .unwrap_or_default()
}

pub fn cvt_class_id_to_type(class_id: i32) -> String {
    match class_id {
        6 => "mod".to_string(),
        12 => "resourcepack".to_string(),
        17 => "world".to_string(),
        4471 => "modpack".to_string(),
        6552 => "shader".to_string(),
        6945 => "datapack".to_string(),
        _ => "unknown".to_string(),
    }
}

pub fn cvt_type_to_class_id(_type: &str) -> u32 {
    match _type {
        "mod" => 6,
        "resourcepack" => 12,
        "world" => 17,
        "modpack" => 4471,
        "shader" => 6552,
        "datapack" => 6945,
        _ => 0,
    }
}

pub fn cvt_sort_by_to_id(sort_by: &str) -> u32 {
    match sort_by {
        "Popularity" => 2,
        "A-Z" => 4,
        "Latest update" => 3,
        "Creation date" => 11,
        "Total downloads" => 6,
        _ => 2,
    }
}

pub fn cvt_mod_loader_to_id(mod_loader: &str) -> u32 {
    match mod_loader {
        "Forge" => 1,
        "Fabric" => 4,
        "Quilt" => 5,
        "NeoForge" => 6,
        _ => 0,
    }
}

pub fn cvt_id_to_release_type(value: u32) -> String {
    crate::resource::curseforge::release_type_name(value).into()
}
pub fn cvt_id_to_dependency_type(value: u32) -> String {
    crate::resource::curseforge::dependency_type_name(value).into()
}
