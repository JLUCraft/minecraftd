//! Shared official and mirror endpoint selection.
use crate::resource::ResourceError;
use url::Url;
pub type MirrorResult<T> = Result<T, ResourceError>;
#[derive(Eq, Hash, PartialEq, Clone, Copy, Debug)]
pub enum ResourceType {
    VersionManifest,
    VersionManifestV2,
    LauncherMeta,
    Launcher,
    Assets,
    Libraries,
    MojangJava,
    ForgeMaven,
    ForgeMeta,
    ForgeMavenNew,
    ForgeInstall,
    Liteloader,
    OptiFine,
    AuthlibInjector,
    FabricMeta,
    FabricMaven,
    NeoforgeMetaForge,    // old version, only for 1.20.1
    NeoforgeMetaNeoforge, // new version
    NeoforgeInstall,
    NeoforgeMaven,
    QuiltMaven,
    QuiltMeta,
    CleanroomMaven,
    CleanroomMeta,
    CleanroomInstall,
}

#[derive(Eq, Hash, PartialEq, Clone, Copy, Debug)]
pub enum SourceType {
    Official,
    Bmclapi,
}

impl ResourceType {
    pub fn iter() -> impl Iterator<Item = Self> {
        [
            Self::VersionManifest,
            Self::VersionManifestV2,
            Self::LauncherMeta,
            Self::Launcher,
            Self::Assets,
            Self::Libraries,
            Self::MojangJava,
            Self::ForgeMaven,
            Self::ForgeMeta,
            Self::ForgeMavenNew,
            Self::ForgeInstall,
            Self::Liteloader,
            Self::OptiFine,
            Self::AuthlibInjector,
            Self::FabricMeta,
            Self::FabricMaven,
            Self::NeoforgeMetaForge,
            Self::NeoforgeMetaNeoforge,
            Self::NeoforgeInstall,
            Self::NeoforgeMaven,
            Self::QuiltMaven,
            Self::QuiltMeta,
            Self::CleanroomMaven,
            Self::CleanroomMeta,
            Self::CleanroomInstall,
        ]
        .into_iter()
    }
}
impl SourceType {
    pub fn iter() -> impl Iterator<Item = Self> {
        [Self::Official, Self::Bmclapi].into_iter()
    }
}
// https://bmclapidoc.bangbang93.com/
pub fn get_download_api(source: SourceType, resource_type: ResourceType) -> MirrorResult<Url> {
    match source {
        SourceType::Official => match resource_type {
            ResourceType::VersionManifest => Ok(Url::parse(
                "https://launchermeta.mojang.com/mc/game/version_manifest.json",
            )?),
            ResourceType::VersionManifestV2 => Ok(Url::parse(
                "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json",
            )?),
            ResourceType::LauncherMeta => Ok(Url::parse("https://launchermeta.mojang.com/")?),
            ResourceType::Launcher => Ok(Url::parse("https://launcher.mojang.com/")?),
            ResourceType::Assets => Ok(Url::parse("https://resources.download.minecraft.net/")?),
            ResourceType::Libraries => Ok(Url::parse("https://libraries.minecraft.net/")?),
            ResourceType::MojangJava => Ok(Url::parse(
                "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json",
            )?),
            ResourceType::ForgeMaven => Ok(Url::parse("https://files.minecraftforge.net/maven/")?),
            ResourceType::ForgeMavenNew => Ok(Url::parse("https://maven.minecraftforge.net")?),
            ResourceType::ForgeInstall => Ok(Url::parse(
                "https://maven.minecraftforge.net/net/minecraftforge/forge/",
            )?),
            ResourceType::ForgeMeta => Err(ResourceError::NoDownloadApi), // https://github.com/HMCL-dev/HMCL/pull/3259/files
            ResourceType::Liteloader => Ok(Url::parse(
                "https://dl.liteloader.com/versions/versions.json",
            )?),
            ResourceType::OptiFine => Err(ResourceError::NoDownloadApi), //
            ResourceType::AuthlibInjector => Ok(Url::parse("https://authlib-injector.yushi.moe/")?),
            ResourceType::FabricMeta => Ok(Url::parse("https://meta.fabricmc.net/")?),
            ResourceType::FabricMaven => Ok(Url::parse("https://maven.fabricmc.net/")?),
            // https://github.com/HMCL-dev/HMCL/blob/efd088e014bf1c113f7b3fdf73fb983087ae3f5e/HMCLCore/src/main/java/org/jackhuang/hmcl/download/neoforge/NeoForgeOfficialVersionList.java#L28
            ResourceType::NeoforgeMetaForge => Ok(Url::parse(
                "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/forge/",
            )?),
            ResourceType::NeoforgeMetaNeoforge => Ok(Url::parse(
                "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge/",
            )?),
            ResourceType::NeoforgeMaven | ResourceType::NeoforgeInstall => {
                Ok(Url::parse("https://maven.neoforged.net/releases/")?)
            }
            ResourceType::QuiltMaven => {
                Ok(Url::parse("https://maven.quiltmc.org/repository/release/")?)
            }
            ResourceType::QuiltMeta => Ok(Url::parse("https://meta.quiltmc.org/")?),
            ResourceType::CleanroomInstall => Ok(Url::parse(
                "https://hmcl.glavo.site/metadata/cleanroom/files/",
            )?),
            ResourceType::CleanroomMaven => Ok(Url::parse("https://maven.cleanroommc.com/")?),
            ResourceType::CleanroomMeta => Ok(Url::parse(
                "https://hmcl.glavo.site/metadata/cleanroom/index.json",
            )?),
        },
        SourceType::Bmclapi => match resource_type {
            ResourceType::VersionManifest => Ok(Url::parse(
                "https://bmclapi2.bangbang93.com/mc/game/version_manifest.json",
            )?),
            ResourceType::VersionManifestV2 => Ok(Url::parse(
                "https://bmclapi2.bangbang93.com/mc/game/version_manifest_v2.json",
            )?),
            ResourceType::LauncherMeta => Ok(Url::parse("https://bmclapi2.bangbang93.com/")?),
            ResourceType::Launcher => Ok(Url::parse("https://bmclapi2.bangbang93.com/")?),
            ResourceType::Assets => Ok(Url::parse("https://bmclapi2.bangbang93.com/assets/")?),
            ResourceType::Libraries => Ok(Url::parse("https://bmclapi2.bangbang93.com/maven/")?),
            ResourceType::MojangJava => Ok(Url::parse(
                "https://bmclapi2.bangbang93.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json",
            )?),
            ResourceType::ForgeMaven
            | ResourceType::ForgeMavenNew
            | ResourceType::NeoforgeMaven => {
                Ok(Url::parse("https://bmclapi2.bangbang93.com/maven/")?)
            }
            ResourceType::ForgeInstall => Ok(Url::parse(
                "https://bmclapi2.bangbang93.com/forge/download/",
            )?),
            ResourceType::ForgeMeta => Ok(Url::parse("https://bmclapi2.bangbang93.com/forge/")?),
            ResourceType::Liteloader => Ok(Url::parse(
                "https://bmclapi.bangbang93.com/maven/com/mumfrey/liteloader/versions.json",
            )?),
            ResourceType::AuthlibInjector => Ok(Url::parse(
                "https://bmclapi2.bangbang93.com/mirrors/authlib-injector/",
            )?),
            ResourceType::FabricMeta => {
                Ok(Url::parse("https://bmclapi2.bangbang93.com/fabric-meta/")?)
            }
            ResourceType::FabricMaven => Ok(Url::parse("https://bmclapi2.bangbang93.com/maven/")?),
            ResourceType::NeoforgeMetaForge | ResourceType::NeoforgeMetaNeoforge => {
                Ok(Url::parse("https://bmclapi2.bangbang93.com/neoforge/")?)
            }
            ResourceType::NeoforgeInstall => Ok(Url::parse(
                "https://bmclapi2.bangbang93.com/neoforge/version/",
            )?),
            ResourceType::OptiFine => Ok(Url::parse("https://bmclapi2.bangbang93.com/optifine/")?),
            ResourceType::QuiltMaven => Ok(Url::parse("https://bmclapi2.bangbang93.com/maven/")?),
            ResourceType::QuiltMeta => {
                Ok(Url::parse("https://bmclapi2.bangbang93.com/quilt-meta/")?)
            } // seems 'not found'
            ResourceType::CleanroomInstall => Ok(Url::parse(
                "https://alist.8mi.tech/d/mirror/HMCL-Metadata/Auto/cleanroom/files/",
            )?),
            ResourceType::CleanroomMaven => Ok(Url::parse("https://maven.cleanroommc.com/")?),
            ResourceType::CleanroomMeta => Ok(Url::parse(
                "https://alist.8mi.tech/d/mirror/HMCL-Metadata/Auto/cleanroom/index.json",
            )?),
        },
    }
}

pub fn convert_url_source_type(
    url: &Url,
    resource_type: ResourceType,
    src_type: SourceType,
    dst_type: SourceType,
) -> MirrorResult<Url> {
    let url_str = url.as_str();
    let src_api = get_download_api(src_type, resource_type)?;
    let dst_api = get_download_api(dst_type, resource_type)?;
    if url_str.starts_with(src_api.as_str()) {
        Ok(Url::parse(
            url_str
                .replacen(src_api.as_str(), dst_api.as_str(), 1)
                .as_str(),
        )?)
    } else {
        Err(ResourceError::NoDownloadApi)
    }
}

pub fn convert_url_to_target_source(
    url: &Url,
    resource_types: &[ResourceType],
    dst_type: &SourceType,
) -> MirrorResult<Url> {
    let mut normalized = url.clone();
    match url.host_str() {
        Some("piston-meta.mojang.com") => {
            normalized.set_host(Some("launchermeta.mojang.com"))?;
        }
        Some("piston-data.mojang.com") => {
            normalized.set_host(Some("launcher.mojang.com"))?;
        }
        _ => {}
    }
    let url_str = normalized.as_str();
    let resource_candidates = if resource_types.is_empty() {
        ResourceType::iter().collect::<Vec<_>>()
    } else {
        resource_types.to_vec()
    };

    for resource_type in resource_candidates {
        let dst_api = match get_download_api(*dst_type, resource_type) {
            Ok(api) => api,
            Err(_) => continue,
        };

        for src_type in SourceType::iter() {
            if &src_type == dst_type {
                continue;
            }

            if let Ok(src_api) = get_download_api(src_type, resource_type)
                && url_str.starts_with(src_api.as_str())
            {
                let new_url_str = url_str.replacen(src_api.as_str(), dst_api.as_str(), 1);
                return Ok(Url::parse(&new_url_str)?);
            }
        }
    }

    // If no replacement occurred, return the original URL
    Ok(url.clone())
}
