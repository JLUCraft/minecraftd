//! Resource-pack metadata; image decoding belongs to the caller's presentation layer.
use std::{fs, io::Read, path::Path};

#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, PartialEq, Eq)]
pub struct PackMetadata {
    pub description: String,
    pub icon: Option<Vec<u8>>,
}

fn description(contents: &str) -> Result<String, PackError> {
    let value: serde_json::Value = serde_json::from_str(contents.trim_start_matches('\u{feff}'))?;
    Ok(value["pack"]["description"]
        .as_str()
        .unwrap_or_default()
        .to_owned())
}

pub fn read_archive(path: &Path) -> Result<PackMetadata, PackError> {
    let mut archive = zip::ZipArchive::new(fs::File::open(path)?)?;
    let mut contents = String::new();
    archive
        .by_name("pack.mcmeta")?
        .read_to_string(&mut contents)?;
    let description = description(&contents)?;
    let icon = match archive.by_name("pack.png") {
        Ok(mut entry) => {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            Some(bytes)
        }
        Err(zip::result::ZipError::FileNotFound) => None,
        Err(error) => return Err(error.into()),
    };
    Ok(PackMetadata { description, icon })
}

pub async fn read_directory(path: &Path) -> Result<PackMetadata, PackError> {
    let description = description(&tokio::fs::read_to_string(path.join("pack.mcmeta")).await?)?;
    let icon = match tokio::fs::read(path.join("pack.png")).await {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    Ok(PackMetadata { description, icon })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn directory_and_archive_use_same_metadata() {
        let root = tempfile::tempdir().unwrap();
        let metadata = "\u{feff}{\"pack\":{\"description\":\"A pack\",\"pack_format\":15}}";
        fs::write(root.path().join("pack.mcmeta"), metadata).unwrap();
        fs::write(root.path().join("pack.png"), b"icon bytes").unwrap();
        let archive_path = root.path().join("pack.zip");
        let mut archive = zip::ZipWriter::new(fs::File::create(&archive_path).unwrap());
        for (name, bytes) in [
            ("pack.mcmeta", metadata.as_bytes()),
            ("pack.png", b"icon bytes".as_slice()),
        ] {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(bytes).unwrap();
        }
        archive.finish().unwrap();
        assert_eq!(
            read_archive(&archive_path).unwrap(),
            read_directory(root.path()).await.unwrap()
        );
        fs::write(root.path().join("pack.mcmeta"), "invalid").unwrap();
        assert!(matches!(
            read_directory(root.path()).await,
            Err(PackError::Json(_))
        ));
    }
}
