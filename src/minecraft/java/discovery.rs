//! Java discovery and inspection, independent of an application's configuration store.
use crate::minecraft::java::JavaRuntime;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;
pub const LTS_VERSIONS: &[i32] = &[8, 11, 17, 21, 25];
#[derive(Debug, Default)]
pub struct JavaLocations {
    pub home: Option<PathBuf>,
    pub data: Option<PathBuf>,
    pub game_directories: Vec<PathBuf>,
    pub drives: Vec<PathBuf>,
}

pub fn inspect(java_path: &str, is_user_added: bool) -> Option<JavaRuntime> {
    let (mut vendor, mut version) = get_java_info_from_release_file(java_path);
    if vendor.is_none() || version.is_none() {
        let (command_vendor, command_version) = get_java_info_from_command(java_path);
        vendor = vendor.or(command_vendor);
        version = version.or(command_version);
    }
    let version = version?;
    let (major_version, is_lts) = parse_java_major_version(&version);
    if major_version == 0 {
        return None;
    }
    let compiler = if cfg!(windows) { "javac.exe" } else { "javac" };
    let is_jdk = Path::new(java_path)
        .parent()
        .is_some_and(|path| path.join(compiler).exists());
    Some(JavaRuntime {
        name: format!("{} {version}", if is_jdk { "JDK" } else { "JRE" }),
        exec_path: PathBuf::from(java_path),
        vendor: vendor.unwrap_or_else(|| "Unknown Vendor".to_owned()),
        major_version,
        is_lts,
        is_user_added,
    })
}
pub fn get_java_paths(locations: &JavaLocations) -> Vec<String> {
    let mut paths = HashSet::new();

    // List java paths from System PATH variable
    #[cfg(target_os = "windows")]
    let command_output = Command::new("where")
        .arg("java")
        // See https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags
        .creation_flags(CREATE_NO_WINDOW) // CREATE_NO_WINDOW
        .output();

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let command_output = Command::new("which").args(["-a", "java"]).output();

    if let Ok(output) = command_output
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let path = line.trim();
            if path.is_empty() {
                continue;
            }
            if let Ok(resolved_path) = fs::canonicalize(path) {
                paths.insert(resolved_path.to_string_lossy().into_owned());
            }
        }
    }

    // Scan java paths from common dirs
    for java_path in scan_java_paths_in_common_directories(locations) {
        paths.insert(java_path);
    }

    // Scan Mojang Java downloaded by SJMCL itself
    for java_path in scan_java_paths_in_runtime_directory(locations) {
        paths.insert(java_path);
    }

    // Scan java paths in all configured game directories (may downloaded by PCL)
    for java_path in scan_java_paths_in_game_directories(locations) {
        paths.insert(java_path);
    }

    // For windows, try to get java path from registry
    #[cfg(target_os = "windows")]
    {
        for java_path in scan_java_paths_in_windows_registry() {
            paths.insert(java_path);
        }
    }

    // For macOS, run "/usr/libexec/java_home -V" additionally
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = Command::new("/usr/libexec/java_home").arg("-V").output()
            && output.status.success()
        {
            let stderr = String::from_utf8_lossy(&output.stderr);
            for line in stderr.lines() {
                let trimmed = line.trim();
                // Don't use `split_whitespace().last()` because the path may contain spaces.
                if let Some(idx) = trimmed.rfind('"') {
                    let path_part = &trimmed[idx + 1..].trim();
                    if !path_part.is_empty() {
                        let java_bin = PathBuf::from(path_part).join("bin/java");
                        if let Ok(resolved_path) = fs::canonicalize(java_bin) {
                            paths.insert(resolved_path.to_string_lossy().into_owned());
                        }
                    }
                }
            }
        }
    }

    // For Windows, remove "\\?\" prefix from paths
    #[cfg(target_os = "windows")]
    {
        paths
            .into_iter()
            .map(|path| path.trim_start_matches(r"\\?\").to_string())
            .collect()
    }

    #[cfg(not(target_os = "windows"))]
    {
        paths.into_iter().collect()
    }
}

fn resolve_java_home(path: PathBuf) -> std::io::Result<String> {
    let java_bin = path.join(if cfg!(windows) {
        "bin/java.exe"
    } else {
        "bin/java"
    });
    Ok(fs::canonicalize(java_bin)?.to_string_lossy().into_owned())
}

fn search_java_homes_in_directory(dir: PathBuf) -> Vec<String> {
    let mut java_paths = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let java_home = entry.path();
            if let Ok(java_path) = resolve_java_home(java_home) {
                java_paths.push(java_path);
            }
        }
    }
    java_paths
}

#[cfg(target_os = "macos")]
fn search_java_homes_in_mac_java_virtual_machines(dir: PathBuf) -> Vec<String> {
    let mut java_paths = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let java_home = entry.path().join("Contents/Home");
            if let Ok(java_path) = resolve_java_home(java_home) {
                java_paths.push(java_path);
            }
        }
    }
    java_paths
}

fn scan_java_paths_in_common_directories(locations: &JavaLocations) -> Vec<String> {
    let mut java_paths = Vec::new();
    if let Ok(home_dir) = locations.home.as_ref().ok_or(()) {
        java_paths.extend(search_java_homes_in_directory(home_dir.join(".jdks")));
    }
    #[cfg(target_os = "windows")]
    {
        let common_vendors = [
            "Java",
            "BellSoft",
            "AdoptOpenJDK",
            "SapMachine",
            "Amazon Corretto",
            "Zulu",
            "Microsoft",
            "Eclipse Foundation",
            "Eclipse Adoptium",
            "Semeru",
        ];
        for mount in &locations.drives {
            for vendor in &common_vendors {
                java_paths.extend(search_java_homes_in_directory(
                    mount.join("Program Files").join(vendor),
                ));
                java_paths.extend(search_java_homes_in_directory(
                    mount.join("Program Files (x86)").join(vendor),
                ));
            }
            java_paths.extend(search_java_homes_in_directory(mount.join("Java")));
        }
    }
    #[cfg(target_os = "linux")]
    {
        let common_dirs = [
            "/usr/java",
            "/usr/lib/jvm",
            "/usr/lib32/jvm",
            "/usr/lib64/jvm",
        ];
        for dir in common_dirs {
            java_paths.extend(search_java_homes_in_directory(PathBuf::from(dir)));
        }
        if let Ok(home) = locations.home.as_ref().ok_or(()) {
            java_paths.extend(search_java_homes_in_directory(
                home.join(".sdkman/candidates/java"),
            ));
        }
    }
    #[cfg(target_os = "macos")]
    {
        let common_javas = [
            "/Library/Internet Plug-Ins/JavaAppletPlugin.plugin/Contents/Home",
            "/Applications/Xcode.app/Contents/Applications/Application Loader.app/Contents/MacOS/itms/java",
            "/opt/homebrew/opt/java",
        ];
        for java in common_javas {
            java_paths.extend(resolve_java_home(PathBuf::from(java)));
        }
        java_paths.extend(search_java_homes_in_mac_java_virtual_machines(
            PathBuf::from("/Library/Java/JavaVirtualMachines"),
        ));
        if let Ok(home) = locations.home.as_ref().ok_or(()) {
            java_paths.extend(search_java_homes_in_mac_java_virtual_machines(
                home.join("Library/Java/JavaVirtualMachines"),
            ));
        }
        if let Ok(entries) = fs::read_dir(PathBuf::from("/opt/homebrew/Cellar")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name()
                    && name.to_string_lossy().starts_with("openjdk")
                {
                    java_paths.extend(search_java_homes_in_directory(path));
                }
            }
        }
    }
    java_paths
}

fn scan_java_paths_in_runtime_directory(locations: &JavaLocations) -> Vec<String> {
    let mut java_paths = Vec::new();
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        if let Ok(app_data_dir) = locations.data.as_ref().ok_or(()) {
            java_paths.extend(search_java_homes_in_directory(app_data_dir.join("runtime")));
        }
        // TODO: test on the Linux.
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(rt) = locations.data.as_ref().ok_or(()).map(|p| p.join("runtime")) {
            for v in LTS_VERSIONS {
                java_paths.extend(search_java_homes_in_mac_java_virtual_machines(
                    rt.join(format!("java-{v}")),
                ));
            }
        }
    }
    java_paths
}

fn scan_java_paths_in_game_directories(locations: &JavaLocations) -> Vec<String> {
    let mut java_paths = Vec::new();
    for game_dir in &locations.game_directories {
        java_paths.extend(search_java_homes_in_directory(game_dir.join("runtime")));
    }
    java_paths
}

// Get canonicalized java paths from Windows registry
#[cfg(target_os = "windows")]
fn scan_java_paths_in_windows_registry() -> Vec<String> {
    let registry_paths = [
        r"SOFTWARE\JavaSoft\JDK",
        r"SOFTWARE\JavaSoft\JRE",
        r"SOFTWARE\JavaSoft\Java Development Kit",
        r"SOFTWARE\JavaSoft\Java Runtime Environment",
    ];

    let resolve_registry_path = |base_path: &str| -> Vec<String> {
        let hklm = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE);
        let Ok(base_key) = hklm.open_subkey(base_path) else {
            return Vec::new();
        };

        base_key
            .enum_keys()
            .filter_map(Result::ok)
            .filter(|version_name| {
                version_name
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit())
            })
            .filter_map(|version_name| {
                base_key
                    .open_subkey(&version_name)
                    .and_then(|key| key.get_value::<String, _>("JavaHome"))
                    .ok()
            })
            .collect()
    };

    registry_paths
        .iter()
        .flat_map(|&base_path| resolve_registry_path(base_path))
        .filter_map(|java_home| resolve_java_home(PathBuf::from(java_home)).ok())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect()
}

pub fn get_java_info_from_release_file(java_path: &str) -> (Option<String>, Option<String>) {
    // Typically, executable files are located in .../Home/bin/java, release file and bin folder are at the same level.
    let java_path_buf = PathBuf::from(java_path);
    let java_home = match java_path_buf.parent().and_then(|p| p.parent()) {
        Some(p) => p,
        None => return (None, None),
    };
    let release_file = java_home.join("release");

    if !release_file.exists() {
        return (None, None);
    }
    let content = match fs::read_to_string(release_file) {
        Ok(c) => c,
        Err(_) => return (None, None),
    };

    let mut vendor = None;
    let mut full_version = None;

    // Try to parse info from release file
    for line in content.lines() {
        if line.starts_with("JAVA_VERSION=") {
            if let Some(val) = line.split('=').nth(1) {
                full_version = Some(val.trim().trim_matches('"').to_string());
            }
        } else if line.starts_with("IMPLEMENTOR=")
            && let Some(val) = line.split('=').nth(1)
        {
            vendor = Some(val.trim().trim_matches('"').to_string());
        }
    }

    (vendor, full_version)
}

pub fn get_java_info_from_command(java_path: &str) -> (Option<String>, Option<String>) {
    // use "java -version -XshowSettings:properties" command to get info
    #[cfg(target_os = "windows")]
    let output = Command::new(java_path)
        .args(["-XshowSettings:properties", "-version"])
        .creation_flags(CREATE_NO_WINDOW) // CREATE_NO_WINDOW
        .output();
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let output = Command::new(java_path)
        .args(["-XshowSettings:properties", "-version"])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return (None, None),
    };

    let mut output_str = String::new();
    output_str.push_str(&String::from_utf8_lossy(&output.stdout));
    output_str.push_str(&String::from_utf8_lossy(&output.stderr));

    let mut vendor = None;
    let mut full_version = None;

    for line in output_str.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("java.vendor = ")
            && let Some(val) = line.split('=').nth(1)
        {
            vendor = Some(val.trim().trim_matches('"').to_string());
        }
        if trimmed.starts_with("java.version = ")
            && let Some(val) = line.split('=').nth(1)
        {
            full_version = Some(val.trim().trim_matches('"').to_string());
        }
    }

    (vendor, full_version)
}

pub fn parse_java_major_version(full_version: &str) -> (i32, bool) {
    let major_version = if full_version.starts_with("1.") {
        // Java 1.x (e.g., 1.8 -> 8, 1.7 -> 7)
        full_version
            .split('.')
            .nth(1)
            .unwrap_or("0")
            .parse::<i32>()
            .unwrap_or(0)
    } else {
        // Java 9+
        full_version
            .split(['.', '-', '+'])
            .next()
            .unwrap_or("0")
            .parse::<i32>()
            .unwrap_or(0)
    };

    let is_lts = LTS_VERSIONS.contains(&major_version);
    (major_version, is_lts)
}
