use std::io::ErrorKind;
use std::path::Path;
use uuid::Uuid;

use crate::minecraft::nbt::nbt_to_display_value;
use crate::minecraft::world::base::WorldInfo;

use quartz_nbt::io::{Flavor, read_nbt};
use quartz_nbt::{NbtCompound, NbtTag};
#[derive(Debug, thiserror::Error)]
pub enum WorldError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("NBT error: {0}")]
    Nbt(#[from] quartz_nbt::io::NbtIoError),
    #[error("invalid NBT metadata: {0}")]
    Metadata(#[from] quartz_nbt::NbtReprError),
    #[error("invalid world metadata: {0}")]
    Invalid(String),
}
pub type WorldResult<T> = Result<T, WorldError>;
pub type WorldDetails = std::collections::HashMap<String, serde_json::Value>;
pub mod base;
pub async fn load_world_info_from_dir(
    path: &Path,
    has_difficulty_support: bool,
) -> WorldResult<WorldInfo> {
    // World-list metadata is stored in level.dat.
    let level = load_level_nbt(&path.join("level.dat")).await?;
    let (last_played, difficulty, gamemode) =
        level_data_to_world_info(level.get::<_, &NbtCompound>("Data")?)?;

    Ok(WorldInfo {
        name: path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string(),
        last_played_at: last_played,
        difficulty: has_difficulty_support.then_some(difficulty),
        gamemode,
        icon_src: path.join("icon.png"),
        dir_path: path.to_path_buf(),
    })
}

async fn read_nbt_file(path: &Path) -> WorldResult<Option<NbtCompound>> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    Ok(Some(
        read_nbt(&mut bytes.as_slice(), Flavor::GzCompressed)?.0,
    ))
}

async fn load_level_nbt(path: &Path) -> WorldResult<NbtCompound> {
    let level = read_nbt_file(path).await?.ok_or_else(|| {
        std::io::Error::new(
            ErrorKind::NotFound,
            format!("world metadata is missing: {}", path.display()),
        )
    })?;
    level.get::<_, &NbtCompound>("Data")?;
    Ok(level)
}

pub async fn load_world_data_from_dir(path: &Path) -> WorldResult<WorldDetails> {
    // The Data compound and external files are returned as separate display entries.
    // https://minecraft.wiki/w/Java_Edition_level_format
    let level = load_level_nbt(&path.join("level.dat")).await?;
    let data = level.get::<_, &NbtCompound>("Data")?;

    // Optional metadata files in the 26.1+ layout.
    let mut files = vec![
        "data/minecraft/world_gen_settings.dat".to_string(),
        "data/minecraft/weather.dat".to_string(),
        "data/minecraft/game_rules.dat".to_string(),
        "data/minecraft/wandering_trader.dat".to_string(),
        "dimensions/minecraft/overworld/data/minecraft/world_border.dat".to_string(),
    ];

    // 26.1+ replaces Player with singleplayer_uuid, referencing players/data/<uuid>.dat.
    // https://minecraft.wiki/w/Player.dat_format
    if !data.contains_key("Player")
        && let Some(tag) = data.inner().get("singleplayer_uuid")
    {
        match tag {
            NbtTag::IntArray(parts) if parts.len() == 4 => {
                // Preserve the bits of the four signed UUID words, most significant first.
                let most = ((parts[0] as u32 as u64) << 32) | parts[1] as u32 as u64;
                let least = ((parts[2] as u32 as u64) << 32) | parts[3] as u32 as u64;
                files.push(format!(
                    "players/data/{}.dat",
                    Uuid::from_u64_pair(most, least)
                ));
            }
            _ => tracing::warn!(path = %path.display(), "invalid singleplayer_uuid"),
        }
    }

    let mut details = WorldDetails::from([(
        "level.dat".to_string(),
        nbt_to_display_value(NbtTag::Compound(level)),
    )]);
    for file in files {
        let file_path = path.join(&file);
        match read_nbt_file(&file_path).await {
            Ok(Some(data)) => {
                details.insert(file, nbt_to_display_value(NbtTag::Compound(data)));
            }
            Ok(None) if file.starts_with("players/") => {
                tracing::warn!(path = %file_path.display(), "missing world player data");
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(path = %file_path.display(), %error, "failed to read world data")
            }
        }
    }
    Ok(details)
}

fn level_data_to_world_info(data: &NbtCompound) -> WorldResult<(i64, String, String)> {
    // Read the difficulty tags stored by the world.
    let settings = data
        .inner()
        .get("difficulty_settings")
        .map(<&NbtCompound>::try_from)
        .transpose()?;
    let hardcore = settings
        .and_then(|s| s.inner().get("hardcore"))
        .or_else(|| data.inner().get("hardcore"))
        .map(bool::try_from)
        .transpose()?
        .unwrap_or(false);

    const DIFFICULTIES: [&str; 4] = ["peaceful", "easy", "normal", "hard"];
    let difficulty = if let Some(tag) = settings.and_then(|s| s.inner().get("difficulty")) {
        let value = <&str>::try_from(tag)?;
        if !DIFFICULTIES.contains(&value) {
            return Err(WorldError::Invalid(format!(
                "Invalid world difficulty: {}",
                value
            )));
        }
        value
    } else {
        let value = data
            .inner()
            .get("Difficulty")
            .map(u8::try_from)
            .transpose()?
            .unwrap_or(2);
        DIFFICULTIES
            .get(value as usize)
            .copied()
            .ok_or_else(|| WorldError::Invalid(format!("Invalid world difficulty: {}", value)))?
    };

    const GAMEMODES: [&str; 4] = ["survival", "creative", "adventure", "spectator"];
    // Worlds predating game modes have no GameType and are always survival.
    let game_type = data
        .inner()
        .get("GameType")
        .map(i32::try_from)
        .transpose()?
        .unwrap_or(0);
    let gamemode = GAMEMODES
        .get(game_type as usize)
        .ok_or_else(|| WorldError::Invalid(format!("Invalid world game type: {}", game_type)))?;

    Ok((
        data.get::<_, i64>("LastPlayed")? / 1000,
        if hardcore { "hardcore" } else { difficulty }.to_string(),
        gamemode.to_string(),
    ))
}
