//! Version profiles, patch composition and launch rules shared by all clients.
use crate::minecraft::loaders::ModLoaderType;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::str::FromStr;
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("invalid version rule: {0}")]
    Rule(String),
    #[error(transparent)]
    Regex(#[from] regex::Error),
}
type ProfileResult<T> = Result<T, ProfileError>;
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RulePlatform {
    pub os: String,
    pub arch: String,
    pub version: String,
}
impl Default for RulePlatform {
    fn default() -> Self {
        Self {
            os: std::env::consts::OS.into(),
            // This field uses the rule protocol's architecture token, not a host triple.
            arch: if cfg!(target_arch = "x86_64") {
                "x64"
            } else {
                std::env::consts::ARCH
            }
            .into(),
            version: String::new(),
        }
    }
}
fn one_or_many<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Input {
        One(String),
        Many(Vec<String>),
    }
    Ok(match Input::deserialize(deserializer)? {
        Input::One(value) => vec![value],
        Input::Many(values) => values,
    })
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptiFineResourceInfo {
    pub filename: String,
    pub patch: String,
    pub r#type: String,
}
#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct McClientInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compliance_level: Option<u32>,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inherits_from: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<LaunchArgumentTemplate>, // new version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minecraft_arguments: Option<String>, // old version

    pub asset_index: AssetIndexInfo,
    pub assets: String,
    pub downloads: HashMap<String, DownloadsValue>,
    pub libraries: Vec<LibrariesValue>,
    pub logging: Logging,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub java_version: Option<JavaVersion>,
    #[serde(rename = "type")]
    pub type_: String,
    pub time: String,
    pub release_time: String,
    pub minimum_launcher_version: i64,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub patches: Vec<Self>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_version: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct JavaVersion {
    pub component: String,
    pub major_version: i32,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct LaunchArgumentTemplate {
    pub game: Vec<ArgumentsItem>,
    pub jvm: Vec<ArgumentsItem>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ArgumentsItem {
    pub value: Vec<String>,
    pub rules: Vec<InstructionRule>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct ArgumentsItemDefault {
    #[serde(deserialize_with = "one_or_many")]
    pub value: Vec<String>,
    pub rules: Vec<InstructionRule>,
}

impl<'de> Deserialize<'de> for ArgumentsItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw_value: Value = Value::deserialize(deserializer)?;
        if let Some(val) = raw_value.as_str() {
            return Ok(Self {
                value: vec![val.to_string()],
                ..Default::default()
            });
        }
        let game: ArgumentsItemDefault =
            serde::de::Deserialize::deserialize(raw_value).map_err(serde::de::Error::custom)?;
        Ok(Self {
            value: game.value,
            rules: game.rules,
        })
    }
}

impl Serialize for ArgumentsItem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if self.rules.is_empty() {
            if self.value.len() == 1 {
                return serializer.serialize_str(&self.value[0]);
            }
            if self.value.is_empty() {
                return serializer.serialize_str("");
            }
        }
        let game = ArgumentsItemDefault {
            value: self.value.clone(),
            rules: self.rules.clone(),
        };
        game.serialize(serializer)
    }
}

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", default)]
pub struct InstructionRule {
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub features: Option<FeaturesInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<OsInfo>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", default)]
pub struct OsInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq, Eq, Hash)]
#[serde(default)]
pub struct FeaturesInfo {
    #[serde(skip)]
    pub platform: RulePlatform,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_demo_user: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_custom_resolution: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_quick_plays_support: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_quick_play_singleplayer: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_quick_play_multiplayer: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_quick_play_realms: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct AssetIndexInfo {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub total_size: u64,
    pub url: String,
}

pub type DownloadsValue = DownloadsArtifact;

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct LibrariesValue {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloads: Option<LibrariesDownloads>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub natives: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extract: Option<LibrariesExtract>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub rules: Vec<InstructionRule>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct LibrariesDownloads {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<DownloadsArtifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classifiers: Option<HashMap<String, DownloadsArtifact>>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct LibrariesExtract {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", default)]
pub struct DownloadsArtifact {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    pub url: String,
    pub sha1: String,
    pub size: u64,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Logging {
    pub client: LoggingClient,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct LoggingClient {
    pub argument: String,
    pub file: LoggingFile,
    #[serde(rename = "type")]
    pub type_: String,
}

#[derive(Debug, Deserialize, Serialize, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct LoggingFile {
    pub id: String,
    pub url: String,
    pub sha1: String,
    pub size: u64,
}

pub fn patches_to_info(
    patches: &[McClientInfo],
) -> (
    Option<String>,
    Option<String>,
    ModLoaderType,
    Option<OptiFineResourceInfo>,
) {
    let mut loader_type = ModLoaderType::Unknown;
    let mut game_version = None;
    let mut loader_version = None;
    let mut optifine_info: Option<OptiFineResourceInfo> = None;
    for patch in patches {
        if game_version.is_none() && patch.id == "game" {
            game_version = patch.version.clone();
        }
        if loader_type == ModLoaderType::Unknown
            && let Ok(found_loader_type) = ModLoaderType::from_str(&patch.id)
        {
            loader_type = found_loader_type;
            loader_version = patch.version.clone();
        }
        if patch.id == "optifine" {
            optifine_info = Some(OptiFineResourceInfo {
                patch: "".to_string(),
                filename: "".to_string(),
                r#type: patch.version.clone().unwrap_or_default(),
            });
        }
    }
    (game_version, loader_version, loader_type, optifine_info)
}

pub fn remove_mod_loader_from_client_info(
    client_info: &mut McClientInfo,
    loader_type: ModLoaderType,
) {
    if loader_type == ModLoaderType::Unknown {
        return;
    }

    let is_target_loader_patch = |patch: &McClientInfo| {
        ModLoaderType::from_str(&patch.id)
            .map(|patch_loader_type| match loader_type {
                ModLoaderType::Forge | ModLoaderType::LegacyForge => {
                    matches!(
                        patch_loader_type,
                        ModLoaderType::Forge | ModLoaderType::LegacyForge
                    )
                }
                _ => patch_loader_type == loader_type,
            })
            .unwrap_or(false)
    };

    client_info
        .patches
        .retain(|patch| !is_target_loader_patch(patch));

    reset_fields_from_patches(client_info);
}

pub fn remove_optifine_from_client_info(client_info: &mut McClientInfo) {
    client_info.patches.retain(|patch| patch.id != "optifine");
    reset_fields_from_patches(client_info);
}

pub fn reset_fields_from_patches(client_info: &mut McClientInfo) {
    let patches = client_info.patches.clone();
    let Some(base) = patches.first() else {
        return;
    };
    let has_forge_patch = patches
        .iter()
        .any(|patch| matches!(patch.id.as_str(), "cleanroom" | "forge" | "legacyforge"));

    client_info.libraries.clear();
    for patch in &patches {
        client_info.libraries = crate::minecraft::dependencies::merge_library_lists(
            &client_info.libraries,
            &patch.libraries,
        );
    }

    client_info.main_class = base.main_class.clone();
    client_info.arguments = base.arguments.clone();
    client_info.minecraft_arguments = base.minecraft_arguments.clone();

    let mut patch_refs = patches.iter().skip(1).collect::<Vec<_>>();
    patch_refs.sort_by_key(|patch| patch.priority.unwrap_or(i64::MIN));

    // Non-OptiFine `minecraftArguments` replace the inherited template; OptiFine arguments are incremental (ref: #1897).
    if let Some(minecraft_arguments) = patch_refs
        .iter()
        .rev()
        .filter(|patch| patch.id != "optifine")
        .filter_map(|patch| patch.minecraft_arguments.as_ref())
        .find(|arguments| !arguments.is_empty())
    {
        client_info.minecraft_arguments = Some(minecraft_arguments.clone());
    }

    for patch in patch_refs {
        if let Some(main_class) = &patch.main_class {
            client_info.main_class = Some(main_class.clone());
        }

        let mut patch_arguments = patch.arguments.clone();
        if patch.id == "optifine"
            && let Some(arguments) = &mut patch_arguments
        {
            normalize_optifine_tweaker(arguments, has_forge_patch);
        }

        if let Some(arguments) = patch_arguments {
            if let Some(base_arguments) = &mut client_info.arguments {
                base_arguments.game.extend(arguments.game.clone());
                base_arguments.jvm.extend(arguments.jvm.clone());
            } else {
                client_info.arguments = Some(arguments.clone());
            }
            client_info.minecraft_arguments = None;
        }

        let mut patch_minecraft_arguments = patch.minecraft_arguments.clone();
        if patch.id == "optifine"
            && let Some(minecraft_arguments) = &mut patch_minecraft_arguments
        {
            normalize_optifine_tweaker_str(minecraft_arguments, has_forge_patch);
        }

        if let Some(minecraft_arguments) = &patch_minecraft_arguments {
            if minecraft_arguments.is_empty() {
                continue;
            }

            if patch.id != "optifine" {
                continue;
            }

            match &mut client_info.minecraft_arguments {
                Some(base_arguments) if !base_arguments.is_empty() => {
                    if !base_arguments.ends_with(' ') {
                        base_arguments.push(' ');
                    }
                    base_arguments.push_str(minecraft_arguments);
                }
                Some(base_arguments) => base_arguments.push_str(minecraft_arguments),
                None => client_info.minecraft_arguments = Some(minecraft_arguments.clone()),
            }
        }
    }
}

fn normalize_optifine_tweaker(arguments: &mut LaunchArgumentTemplate, has_forge_patch: bool) {
    let tweaker = if has_forge_patch {
        "optifine.OptiFineForgeTweaker"
    } else {
        "optifine.OptiFineTweaker"
    };

    for item in &mut arguments.game {
        for value in &mut item.value {
            if matches!(
                value.as_str(),
                "optifine.OptiFineTweaker" | "optifine.OptiFineForgeTweaker"
            ) {
                *value = tweaker.to_string();
            }
        }
    }
}

fn normalize_optifine_tweaker_str(arguments: &mut String, has_forge_patch: bool) {
    let tweaker = if has_forge_patch {
        "optifine.OptiFineForgeTweaker"
    } else {
        "optifine.OptiFineTweaker"
    };

    *arguments = arguments
        .replace("optifine.OptiFineForgeTweaker", tweaker)
        .replace("optifine.OptiFineTweaker", tweaker);
}

pub fn libraries_to_info(
    client: &McClientInfo,
) -> (
    Option<String>,
    Option<String>,
    ModLoaderType,
    Option<OptiFineResourceInfo>,
) {
    let game_version: Option<String> = client.client_version.clone();
    let mut loader_version: Option<String> = None;
    let mut loader_type = ModLoaderType::Unknown;

    for lib in &client.libraries {
        let parts: Vec<_> = lib.name.splitn(3, ':').collect();
        if parts.len() != 3 {
            continue;
        }
        let (g, a, v) = (parts[0], parts[1], parts[2]);
        match (g, a) {
            ("net.fabricmc", "fabric-loader") => {
                loader_type = ModLoaderType::Fabric;
                loader_version = Some(v.to_string());
                break;
            }
            ("org.quiltmc", "quilt-loader") => {
                loader_type = ModLoaderType::Quilt;
                loader_version = Some(v.to_string());
                break;
            }
            ("com.cleanroommc", "cleanroom") => {
                loader_type = ModLoaderType::Cleanroom;
                loader_version = Some(v.to_string());
                break;
            }
            ("net.neoforged.fancymodloader", _) | ("net.neoforged", "fancymodloader") => {
                loader_type = ModLoaderType::NeoForge;
                // ref: https://github.com/HMCL-dev/HMCL/pull/3638/files
                if let Some(args) = &client.arguments {
                    for [item, next] in args.game.array_windows::<2>() {
                        if let Some(option) = item.value.first()
                            && (option.contains("--fml.neoForgeVersion")
                                || option.contains("--fml.forgeVersion"))
                            && let Some(version) = next.value.first()
                            && !version.is_empty()
                        {
                            loader_version = Some(version.clone());
                        }
                    }
                }
                break;
            }
            ("net.minecraftforge", "forge") | ("net.minecraftforge", "fmlloader") => {
                loader_type = ModLoaderType::Forge;
                if let Some((_, forge)) = v.split_once('-') {
                    loader_version = Some(forge.to_string());
                } else {
                    loader_version = Some(v.to_string());
                }
                break;
            }
            ("com.mumfrey", "liteloader") => {
                loader_type = ModLoaderType::LiteLoader;
                loader_version = Some(v.to_string());
                break;
            }
            _ => {}
        }
    }

    (game_version, loader_version, loader_type, None)
}

pub(crate) fn rules_is_allowed(
    rules: &[InstructionRule],
    feature: &FeaturesInfo,
) -> ProfileResult<bool> {
    let mut allowed = rules.is_empty();
    for rule in rules {
        let action = match rule.action.as_str() {
            "allow" => true,
            "disallow" => false,
            other => return Err(ProfileError::Rule(other.into())),
        };
        if let Some(os) = &rule.os {
            let name = if os.name == "osx" { "macos" } else { &os.name };
            if !name.is_empty() && name != "unknown" && name != feature.platform.os {
                continue;
            }
            if let Some(arch) = &os.arch
                && arch != "unknown"
                && arch != &feature.platform.arch
            {
                continue;
            }
            if let Some(version) = &os.version
                && !regex::Regex::new(version)?.is_match(&feature.platform.version)
            {
                continue;
            }
        }
        if let Some(required) = &rule.features {
            let pairs = [
                (required.is_demo_user, feature.is_demo_user),
                (
                    required.has_custom_resolution,
                    feature.has_custom_resolution,
                ),
                (
                    required.has_quick_plays_support,
                    feature.has_quick_plays_support,
                ),
                (
                    required.is_quick_play_singleplayer,
                    feature.is_quick_play_singleplayer,
                ),
                (
                    required.is_quick_play_multiplayer,
                    feature.is_quick_play_multiplayer,
                ),
                (required.is_quick_play_realms, feature.is_quick_play_realms),
            ];
            if pairs.into_iter().any(|(expected, actual)| {
                expected.is_some_and(|value| value != actual.unwrap_or(false))
            }) {
                continue;
            }
        }
        allowed = action;
    }
    Ok(allowed)
}
pub trait IsAllowed {
    fn is_allowed(&self, feature: &FeaturesInfo) -> ProfileResult<bool>;
}
impl IsAllowed for ArgumentsItem {
    fn is_allowed(&self, feature: &FeaturesInfo) -> ProfileResult<bool> {
        rules_is_allowed(&self.rules, feature)
    }
}
impl IsAllowed for LibrariesValue {
    fn is_allowed(&self, feature: &FeaturesInfo) -> ProfileResult<bool> {
        rules_is_allowed(&self.rules, feature)
    }
}
impl LaunchArgumentTemplate {
    pub fn to_jvm_arguments(&self, feature: &FeaturesInfo) -> ProfileResult<Vec<String>> {
        resolve(&self.jvm, feature)
    }
    pub fn to_game_arguments(&self, feature: &FeaturesInfo) -> ProfileResult<Vec<String>> {
        resolve(&self.game, feature)
    }
}
fn resolve(items: &[ArgumentsItem], feature: &FeaturesInfo) -> ProfileResult<Vec<String>> {
    let mut result = Vec::new();
    for item in items {
        if item.is_allowed(feature)? {
            result.extend(item.value.clone());
        }
    }
    Ok(result)
}

impl From<String> for ArgumentsItem {
    fn from(value: String) -> Self {
        Self {
            value: vec![value],
            rules: Vec::new(),
        }
    }
}

impl McClientInfo {
    /// Record the original game profile as the base patch before installing loaders.
    pub fn initialize_client(&mut self, name: &str, version: &str) {
        self.id = name.to_owned();
        self.jar = Some(name.to_owned());
        let mut vanilla = self.clone();
        vanilla.patches.clear();
        vanilla.id = "game".into();
        vanilla.version = Some(version.to_owned());
        vanilla.inherits_from = None;
        vanilla.priority = Some(0);
        self.patches.push(vanilla);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rules_require_all_features_and_last_matching_rule_wins() {
        let mut item: ArgumentsItem = serde_json::from_str(r#"{"rules":[{"action":"allow","features":{"has_quick_plays_support":true,"is_quick_play_multiplayer":true}}],"value":["--quickPlayMultiplayer","${quickPlayMultiplayer}"]}"#).unwrap();
        let mut features = FeaturesInfo {
            has_quick_plays_support: Some(true),
            ..Default::default()
        };
        assert!(!item.is_allowed(&features).unwrap());
        features.is_quick_play_multiplayer = Some(true);
        assert!(item.is_allowed(&features).unwrap());
        item.rules.push(InstructionRule {
            action: "disallow".into(),
            ..Default::default()
        });
        assert!(!item.is_allowed(&features).unwrap());
        item.rules[0].action = "unknown".into();
        assert!(item.is_allowed(&features).is_err());
    }
    #[test]
    fn rule_platform_is_explicit_and_malformed_regex_is_error() {
        let mut rule: InstructionRule = serde_json::from_str(
            r#"{"action":"allow","os":{"name":"osx","arch":"aarch64","version":"^14\\."}}"#,
        )
        .unwrap();
        let features = FeaturesInfo {
            platform: RulePlatform {
                os: "macos".into(),
                arch: "aarch64".into(),
                version: "14.2".into(),
            },
            ..Default::default()
        };
        assert!(rules_is_allowed(&[rule.clone()], &features).unwrap());
        rule.os.as_mut().unwrap().version = Some("[".into());
        assert!(rules_is_allowed(&[rule], &features).is_err());
    }
    #[test]
    fn shared_profile_roundtrips_and_composes_optifine_with_cleanroom() {
        let mut profile: McClientInfo = serde_json::from_str(r#"{"id":"example","javaVersion":{"component":"java-runtime","majorVersion":21},"patches":[{"id":"game","mainClass":"base.Main","minecraftArguments":"--username ${auth_player_name}"},{"id":"cleanroom","priority":30000,"mainClass":"net.minecraft.launchwrapper.Launch","minecraftArguments":"--tweakClass cleanroom.Tweaker"},{"id":"optifine","priority":40000,"minecraftArguments":"--tweakClass optifine.OptiFineTweaker"}]}"#).unwrap();
        assert_eq!(
            serde_json::from_str::<McClientInfo>(&serde_json::to_string(&profile).unwrap())
                .unwrap(),
            profile
        );
        reset_fields_from_patches(&mut profile);
        let args = profile.minecraft_arguments.as_ref().unwrap();
        assert!(args.contains("cleanroom.Tweaker"));
        assert!(args.contains("optifine.OptiFineForgeTweaker"));
        remove_optifine_from_client_info(&mut profile);
        assert!(
            !profile
                .minecraft_arguments
                .as_ref()
                .unwrap()
                .contains("optifine")
        );
    }
}
#[test]
#[cfg(target_arch = "x86_64")]
fn host_platform_uses_the_rule_architecture_token() {
    let platform = RulePlatform::default();
    assert_eq!(platform.arch, "x64");
    let rule: InstructionRule =
        serde_json::from_str(r#"{"action":"allow","os":{"arch":"x64"}}"#).unwrap();
    assert!(
        rules_is_allowed(
            &[rule],
            &FeaturesInfo {
                platform,
                ..Default::default()
            }
        )
        .unwrap()
    );
}
