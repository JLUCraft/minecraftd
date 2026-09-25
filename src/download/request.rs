use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use url::Url;
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DownloadParam {
    pub src: Url,
    pub dest: PathBuf,
    pub filename: Option<String>,
    pub sha1: Option<String>,
}
