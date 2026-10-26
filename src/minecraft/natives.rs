//! Platform native replacements shared by client hosts.
use crate::minecraft::profile::{LibrariesValue, McClientInfo, RulePlatform};
use std::collections::HashMap;
pub type NativeReplacements = HashMap<String, HashMap<String, Option<LibrariesValue>>>;
pub fn native_replacements() -> Result<NativeReplacements, serde_json::Error> {
    serde_json::from_str(include_str!("natives.json"))
}
#[must_use]
pub fn needs_native_replacements(platform: &RulePlatform, newer_than_1201: bool) -> bool {
    if platform.os == "windows"
        || (matches!(platform.arch.as_str(), "x86" | "x64")
            && matches!(platform.os.as_str(), "linux" | "macos"))
    {
        return false;
    }
    !(platform.os == "macos" && platform.arch == "aarch64" && newer_than_1201)
}
pub fn replace_native_libraries(
    client_info: &mut McClientInfo,
    map: &NativeReplacements,
    platform: &RulePlatform,
) {
    let key = format!(
        "{}-{}",
        platform.os.to_lowercase(),
        platform.arch.to_lowercase()
    );
    let Some(platform_map) = map.get(&key) else {
        return;
    };
    for lib in &mut client_info.libraries {
        let key = lib.name.clone();

        if lib.natives.is_some() {
            let natives_key = format!("{key}:natives");
            if let Some(Some(new_lib)) = platform_map.get(&natives_key) {
                lib.name = new_lib.name.clone();
                if new_lib.downloads.is_some() {
                    lib.downloads = new_lib.downloads.clone();
                }
                if new_lib.natives.is_some() {
                    lib.natives = new_lib.natives.clone();
                }
                if new_lib.extract.is_some() {
                    lib.extract = new_lib.extract.clone();
                }
                continue;
            }
        }

        if let Some(Some(new_lib_opt)) = platform_map.get(&key) {
            lib.name = new_lib_opt.name.clone();
            if new_lib_opt.downloads.is_some() {
                lib.downloads = new_lib_opt.downloads.clone();
            }
            if new_lib_opt.natives.is_some() {
                lib.natives = new_lib_opt.natives.clone();
            }
            if new_lib_opt.extract.is_some() {
                lib.extract = new_lib_opt.extract.clone();
            }
        }
    }
}
