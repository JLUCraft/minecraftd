use minecraftd::minecraft::world::{self, WorldError};
use quartz_nbt::{
    NbtCompound, NbtTag,
    io::{Flavor, write_nbt},
};
use std::path::Path;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn write_metadata(path: &Path, root: &NbtCompound) -> TestResult {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut bytes = Vec::new();
    write_nbt(&mut bytes, None, root, Flavor::GzCompressed)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

#[tokio::test]
async fn world_details_preserve_tags_and_javascript_integer_precision() -> TestResult {
    let root = tempfile::tempdir()?;
    let mut data = NbtCompound::new();
    data.insert("LastPlayed", 123_000_i64);
    data.insert("GameType", 1_i32);
    data.insert("Difficulty", 3_i8);
    data.insert("hardcore", 0_i8);
    data.insert("RandomSeed", i64::MIN);
    data.insert("mod:custom", "kept");
    data.insert("LongArray", NbtTag::LongArray(vec![i64::MAX, 42]));
    data.insert("NotFinite", f64::INFINITY);
    let mut level = NbtCompound::new();
    level.insert("Data", data);
    write_metadata(&root.path().join("level.dat"), &level)?;

    let info = world::load_world_info_from_dir(root.path(), true).await?;
    assert_eq!(info.last_played_at, 123);
    assert_eq!(info.gamemode, "creative");
    assert_eq!(info.difficulty.as_deref(), Some("hard"));
    let details = world::load_world_data_from_dir(root.path()).await?;
    assert_eq!(details.len(), 1);
    let data = &details["level.dat"]["Data"];
    assert_eq!(data["RandomSeed"], i64::MIN.to_string());
    assert_eq!(data["LongArray"][0], i64::MAX.to_string());
    assert_eq!(data["LongArray"][1], 42);
    assert_eq!(data["NotFinite"], "inf");
    assert_eq!(data["mod:custom"], "kept");
    assert!(data.get("Player").is_none());
    Ok(())
}

#[tokio::test]
async fn reads_split_world_metadata_and_signed_player_uuid() -> TestResult {
    let root = tempfile::tempdir()?;
    let mut difficulty = NbtCompound::new();
    difficulty.insert("hardcore", 1_i8);
    difficulty.insert("difficulty", "easy");
    let mut data = NbtCompound::new();
    data.insert("LastPlayed", 456_000_i64);
    data.insert("difficulty_settings", difficulty);
    data.insert(
        "singleplayer_uuid",
        NbtTag::IntArray(vec![-1, -1, i32::MIN, 0]),
    );
    let mut level = NbtCompound::new();
    level.insert("Data", data);
    write_metadata(&root.path().join("level.dat"), &level)?;
    let mut external = NbtCompound::new();
    external.insert("marker", "external");
    let player = "players/data/ffffffff-ffff-ffff-8000-000000000000.dat";
    for file in [
        "data/minecraft/weather.dat",
        "data/minecraft/game_rules.dat",
        player,
    ] {
        write_metadata(&root.path().join(file), &external)?;
    }
    // A corrupt optional file must not discard the readable metadata files.
    std::fs::write(
        root.path().join("data/minecraft/world_gen_settings.dat"),
        b"broken",
    )?;
    let info = world::load_world_info_from_dir(root.path(), true).await?;
    assert_eq!(info.last_played_at, 456);
    assert_eq!(info.gamemode, "survival");
    assert_eq!(info.difficulty.as_deref(), Some("hardcore"));
    assert!(
        world::load_world_info_from_dir(root.path(), false)
            .await?
            .difficulty
            .is_none()
    );
    let details = world::load_world_data_from_dir(root.path()).await?;
    assert_eq!(details.len(), 4);
    assert_eq!(details[player]["marker"], "external");
    assert_eq!(details["data/minecraft/weather.dat"]["marker"], "external");
    assert!(!details.contains_key("data/minecraft/world_gen_settings.dat"));
    Ok(())
}

#[tokio::test]
async fn required_world_data_reports_missing_corrupt_and_invalid_metadata() -> TestResult {
    let root = tempfile::tempdir()?;
    assert!(
        matches!(world::load_world_data_from_dir(root.path()).await, Err(WorldError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound)
    );
    std::fs::write(root.path().join("level.dat"), b"broken")?;
    assert!(matches!(
        world::load_world_data_from_dir(root.path()).await,
        Err(WorldError::Nbt(_))
    ));
    write_metadata(&root.path().join("level.dat"), &NbtCompound::new())?;
    assert!(matches!(
        world::load_world_data_from_dir(root.path()).await,
        Err(WorldError::Metadata(_))
    ));
    let mut data = NbtCompound::new();
    data.insert("LastPlayed", 0_i64);
    data.insert("GameType", -1_i32);
    let mut level = NbtCompound::new();
    level.insert("Data", data);
    write_metadata(&root.path().join("level.dat"), &level)?;
    assert!(matches!(
        world::load_world_info_from_dir(root.path(), true).await,
        Err(WorldError::Invalid(_))
    ));
    Ok(())
}
