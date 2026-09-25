//! File layout operations for a Mojang client installation.
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};
#[derive(Debug, thiserror::Error)]
pub enum DirectoryError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("invalid client directory: {0}")]
    Invalid(String),
}
pub fn rename_client(src_version_path: &Path, tgt_name: &str) -> Result<PathBuf, DirectoryError> {
    if tgt_name.is_empty()
        || tgt_name == "."
        || tgt_name == ".."
        || tgt_name.contains(['/', '\\', ':'])
    {
        return Err(DirectoryError::Invalid("invalid instance name".into()));
    }
    if src_version_path.file_name().and_then(|name| name.to_str()) == Some(tgt_name) {
        return Ok(src_version_path.to_owned());
    }
    let src_name = src_version_path
        .file_name()
        .ok_or_else(|| DirectoryError::Invalid("missing instance name".into()))?
        .to_string_lossy()
        .to_string();
    let version_root = src_version_path
        .parent()
        .ok_or_else(|| DirectoryError::Invalid("missing version root".into()))?;
    // rename version directory (if already exists, return conflict error)
    let dst_dir = version_root.join(tgt_name);
    if dst_dir.exists() {
        let mut entries = fs::read_dir(&dst_dir)?;
        if entries.next().is_some() {
            return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists).into());
        }
    }
    fs::rename(src_version_path, &dst_dir)?;

    // rename client jar
    let old_jar = dst_dir.join(format!("{}.jar", src_name));
    let new_jar = dst_dir.join(format!("{}.jar", tgt_name));
    fs::rename(old_jar, new_jar)?;

    // rewrite client json, update "id" field and filename
    let old_json = dst_dir.join(format!("{}.json", src_name));
    let new_json = dst_dir.join(format!("{}.json", tgt_name));
    let mut json_value: Value = serde_json::from_reader(fs::File::open(&old_json)?)?;
    if let Some(obj) = json_value.as_object_mut() {
        obj.insert("id".to_string(), Value::String(tgt_name.to_owned()));
    }
    fs::write(&new_json, json_value.to_string())?;
    fs::remove_file(old_json)?;

    Ok(dst_dir)
}
