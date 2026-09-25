//! Complete client launch policy, independent of a launcher UI or account provider.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shlex::try_quote;
use std::borrow::Cow;
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("invalid launch plan: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LaunchArguments {
    // basic game params
    pub game_assets: String,
    pub assets_root: String,
    pub assets_index_name: String,
    pub game_directory: String,
    pub version_name: String,
    pub version_type: String,
    pub natives_directory: String,
    pub launcher_name: String,
    pub launcher_version: String,

    // compatibility with HMCL
    pub primary_jar_name: String,

    // auth params
    pub auth_access_token: String,
    pub auth_player_name: String,
    pub user_type: String,
    pub auth_uuid: String,
    pub clientid: String,
    pub auth_xuid: String,
    pub user_properties: String,
    pub library_directory: String,
    pub classpath_separator: String,

    pub demo: bool,
    pub resolution_height: u32,
    pub resolution_width: u32,

    // quick play params
    #[serde(rename = "quickPlayPath")]
    pub quick_play_path: String,
    #[serde(rename = "quickPlaySingleplayer")]
    pub quick_play_singleplayer: String,
    #[serde(rename = "quickPlayMultiplayer")]
    pub quick_play_multiplayer: String,
    #[serde(rename = "quickPlayRealms")]
    pub quick_play_realms: String,
}

impl LaunchArguments {
    pub fn into_hashmap(self) -> Result<HashMap<String, String>, PlanError> {
        let value = serde_json::to_value(self).map_err(|e| PlanError::Invalid(e.to_string()))?;
        let obj = value
            .as_object()
            .ok_or_else(|| PlanError::Invalid("invalid launch parameters".into()))?;

        let mut map = HashMap::new();

        for (k, v) in obj.iter() {
            match v {
                Value::Null => {} // skip null or none
                Value::String(s) => {
                    map.insert(k.clone(), s.clone());
                }
                _ => {
                    map.insert(k.clone(), v.to_string());
                }
            }
        }

        Ok(map)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCommand {
    pub class_paths: Vec<String>, // may be too long for Windows, split and use env var
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum GarbageCollector {
    #[default]
    Auto,
    G1gc,
    Zgc,
    Shenandoah,
    Parallel,
    Serial,
}
#[derive(Debug, Clone, Default)]
pub enum Proxy {
    #[default]
    System,
    Http {
        host: String,
        port: u16,
    },
    Socks {
        host: String,
        port: u16,
    },
}
/// Account providers supply the resolved agent and metadata; they never generate JVM flags.
#[derive(Debug, Clone)]
pub struct AuthAgent {
    pub jar: String,
    pub server: String,
    pub prefetched_base64: String,
}
#[derive(Debug, Clone, Default)]
pub struct JvmOptions {
    pub max_memory_mb: u32,
    pub java_major: i32,
    pub permanent_space_mb: u32,
    pub garbage_collector: GarbageCollector,
    pub extra: String,
    pub proxy: Proxy,
    pub client_jar: Option<String>,
    pub auth_agent: Option<AuthAgent>,
    pub unsafe_agent: Option<String>,
    pub platform_args: Vec<String>,
}
#[derive(Debug, Clone, Default)]
pub struct GameOptions {
    pub legacy_server: Option<String>,
    pub fullscreen: bool,
    pub graphics_backend: Option<String>,
    pub extra: String,
}
/// A resolved version profile. Rule evaluation precedes this pure rendering step.
#[derive(Debug, Clone)]
pub struct LaunchPlan {
    pub class_paths: Vec<String>,
    pub parameters: LaunchArguments,
    pub custom_version_name: Option<String>,
    pub main_class: String,
    pub jvm_template: Vec<String>,
    pub game_template: Vec<String>,
    pub jvm: JvmOptions,
    pub game: GameOptions,
}
impl LaunchPlan {
    pub fn render(self) -> Result<LaunchCommand, PlanError> {
        if self.main_class.trim().is_empty() {
            return Err(PlanError::Invalid("missing main class".into()));
        }
        let mut args = self.jvm.render();
        let jvm_map = self.parameters.into_hashmap()?;
        let mut game_map = jvm_map.clone();
        if let Some(name) = self.custom_version_name {
            game_map.insert("version_name".into(), name);
        }
        let mut templates = self.jvm_template;
        // CLASSPATH travels in the process environment to avoid Windows command limits.
        while let Some(index) = templates
            .iter()
            .position(|arg| arg == "-cp" || arg == "-classpath")
        {
            if index + 1 >= templates.len() {
                return Err(PlanError::Invalid("classpath option has no value".into()));
            }
            templates.drain(index..=index + 1);
        }
        args.extend(replace_arguments(templates, &jvm_map));
        args.push(self.main_class);
        args.extend(replace_arguments(self.game_template, &game_map));
        if let Some(server) = self.game.legacy_server.filter(|server| !server.is_empty()) {
            let (host, port) = server.split_once(':').unwrap_or((&server, "25565"));
            args.extend(["--server".into(), host.into(), "--port".into(), port.into()]);
        }
        if self.game.fullscreen {
            args.push("--fullscreen".into());
        }
        if let Some(backend) = self.game.graphics_backend {
            args.extend(["--graphicsBackend".into(), backend]);
        }
        if !self.game.extra.trim().is_empty() {
            args.extend(
                shlex::split(&self.game.extra).ok_or_else(|| {
                    PlanError::Invalid("unmatched quote in game arguments".into())
                })?,
            );
        }
        Ok(LaunchCommand {
            class_paths: self.class_paths,
            args,
        })
    }
}
impl JvmOptions {
    #[must_use]
    pub fn render(&self) -> Vec<String> {
        let mut args = vec![format!("-Xmx{}m", self.max_memory_mb)];
        if self.permanent_space_mb != 0 {
            let name = if self.java_major < 8 {
                "PermSize"
            } else {
                "MetaspaceSize"
            };
            args.push(format!("-XX:{name}={}m", self.permanent_space_mb));
        }
        let collector = match self.garbage_collector {
            GarbageCollector::Auto => None,
            GarbageCollector::G1gc => Some("G1GC"),
            GarbageCollector::Zgc => Some("ZGC"),
            GarbageCollector::Shenandoah => Some("ShenandoahGC"),
            GarbageCollector::Parallel => Some("ParallelGC"),
            GarbageCollector::Serial => Some("SerialGC"),
        };
        if let Some(collector) = collector {
            args.push(format!("-XX:+Use{collector}"));
        }
        args.extend(self.extra.split_whitespace().map(str::to_owned));
        match &self.proxy {
            Proxy::System => args.push("-Djava.net.useSystemProxies=true".into()),
            Proxy::Http { host, port } if !host.trim().is_empty() && *port != 0 => {
                for scheme in ["http", "https"] {
                    args.push(format!("-D{scheme}.proxyHost={}", host.trim()));
                    args.push(format!("-D{scheme}.proxyPort={port}"));
                }
            }
            Proxy::Socks { host, port } if !host.trim().is_empty() && *port != 0 => {
                args.push(format!("-DsocksProxyHost={}", host.trim()));
                args.push(format!("-DsocksProxyPort={port}"));
            }
            _ => {}
        }
        args.push("-Dfile.encoding=UTF-8".into());
        let prefix = if self.java_major < 19 { "sun." } else { "" };
        for stream in ["stdout", "stderr"] {
            args.push(format!("-D{prefix}{stream}.encoding=UTF-8"));
        }
        args.extend([
            "-Djava.rmi.server.useCodebaseOnly=true".into(),
            "-Dcom.sun.jndi.rmi.object.trustURLCodebase=false".into(),
            "-Dcom.sun.jndi.cosnaming.object.trustURLCodebase=false".into(),
        ]);
        args.extend(self.platform_args.clone());
        if let Some(jar) = &self.client_jar {
            args.push(format!("-Dminecraft.client.jar={jar}"));
            #[cfg(target_os = "macos")]
            args.push("-Xdock:name=Minecraft".into());
            args.extend([
                "-Dfml.ignoreInvalidMinecraftCertificates=true".into(),
                "-Dfml.ignorePatchDiscrepancies=true".into(),
            ]);
        }
        if let Some(agent) = &self.auth_agent {
            args.extend([
                format!("-javaagent:{}={}", agent.jar, agent.server),
                "-Dauthlibinjector.side=client".into(),
                format!(
                    "-Dauthlibinjector.yggdrasil.prefetched={}",
                    agent.prefetched_base64
                ),
            ]);
        }
        if let Some(agent) = &self.unsafe_agent {
            args.push(format!("-javaagent:{agent}"));
        }
        args
    }
}
/// Substitute known placeholders once; leave an argument intact if any placeholder is unknown.
#[must_use]
pub fn replace_arguments(args: Vec<String>, values: &HashMap<String, String>) -> Vec<String> {
    args.into_iter()
        .map(|arg| {
            let mut remaining = arg.as_str();
            let mut rendered = String::new();
            while let Some(start) = remaining.find("${") {
                let suffix = &remaining[start + 2..];
                let Some(end) = suffix.find('}') else {
                    break;
                };
                let Some(value) = values.get(&suffix[..end]) else {
                    return arg;
                };
                rendered.push_str(&remaining[..start]);
                rendered.push_str(value);
                remaining = &suffix[end + 1..];
            }
            rendered.push_str(remaining);
            rendered
        })
        .collect()
}
#[must_use]
pub const fn get_separator() -> &'static str {
    if cfg!(windows) { ";" } else { ":" }
}

pub fn export_full_launch_command(
    class_paths: &[String],
    args: &[String],
    java_exec_str: &str,
) -> String {
    fn quote_or_raw(s: &'_ str) -> Cow<'_, str> {
        try_quote(s).unwrap_or(Cow::Borrowed(s))
    }

    let classpath_str = class_paths.join(get_separator());
    let java_exec = quote_or_raw(java_exec_str);
    let mut safe_args = args.to_vec();
    if let Some(access_token_pos) = args.iter().position(|s| s == "--accessToken")
        && access_token_pos + 1 < args.len()
    {
        // avoid leaking access token in exported files
        safe_args[access_token_pos + 1] = "***".to_string();
    }
    let quoted_args = safe_args
        .iter()
        .map(|s| quote_or_raw(s))
        .collect::<Vec<_>>();

    let java_cmd = std::iter::once(java_exec)
        .chain(quoted_args)
        .collect::<Vec<_>>()
        .join(" ");

    if cfg!(windows) {
        format!("set CLASSPATH=\"{}\" && {}", classpath_str, java_cmd)
    } else {
        format!("CLASSPATH=\"{}\" {}", classpath_str, java_cmd)
    }
}

impl LaunchCommand {
    pub fn process_spec(
        self,
        java: &str,
        directory: std::path::PathBuf,
        mut environment: HashMap<String, String>,
        wrapper: &str,
    ) -> Result<crate::core::process::ProcessSpec, PlanError> {
        let mut command = java.to_owned();
        let mut args = Vec::new();
        if !wrapper.trim().is_empty() {
            let parts = shlex::split(wrapper)
                .ok_or_else(|| PlanError::Invalid("invalid wrapper command".into()))?;
            let mut parts = parts.into_iter();
            command = parts
                .next()
                .ok_or_else(|| PlanError::Invalid("empty wrapper command".into()))?;
            args.extend(parts);
            args.push(java.into());
        }
        args.extend(self.args);
        environment.insert("CLASSPATH".into(), self.class_paths.join(get_separator()));
        Ok(crate::core::process::ProcessSpec {
            command,
            args,
            cwd: Some(directory),
            env: environment,
            inherit_env: true,
        })
    }
}

/// LWJGL 3.4 needs the bundled unsafe agent on Java 25 and newer.
#[must_use]
pub fn needs_unsafe_agent(
    profile: &crate::minecraft::profile::McClientInfo,
    java_major: i32,
) -> bool {
    java_major >= 25 && profile.libraries.iter().any(|library| {
        let mut parts = library.name.split(':');
        matches!((parts.next(), parts.next(), parts.next()), (Some(group), Some(name), Some(version)) if group.eq_ignore_ascii_case("org.lwjgl") && name.eq_ignore_ascii_case("lwjgl") && version.starts_with("3.4."))
    })
}

pub fn profile_templates(
    profile: &crate::minecraft::profile::McClientInfo,
    parameters: &LaunchArguments,
    platform: crate::minecraft::profile::RulePlatform,
) -> Result<(Vec<String>, Vec<String>), PlanError> {
    let singleplayer = !parameters.quick_play_singleplayer.trim().is_empty();
    let features = crate::minecraft::profile::FeaturesInfo {
        platform: platform.clone(),
        is_demo_user: Some(parameters.demo),
        has_custom_resolution: Some(true),
        has_quick_plays_support: Some(true),
        is_quick_play_multiplayer: Some(
            !singleplayer && !parameters.quick_play_multiplayer.trim().is_empty(),
        ),
        is_quick_play_singleplayer: Some(singleplayer),
        is_quick_play_realms: Some(!parameters.quick_play_realms.is_empty()),
    };
    if let Some(arguments) = &profile.arguments {
        return Ok((
            arguments
                .to_jvm_arguments(&features)
                .map_err(|error| PlanError::Invalid(error.to_string()))?,
            arguments
                .to_game_arguments(&features)
                .map_err(|error| PlanError::Invalid(error.to_string()))?,
        ));
    }
    let mut jvm = Vec::new();
    if platform.os == "windows" {
        jvm.push("-XX:HeapDumpPath=MojangTricksIntelDriversForPerformance_javaw.exe_minecraft.exe.heapdump".into());
        if platform.version.starts_with("10.") {
            jvm.extend(["-Dos.name=Windows 10".into(), "-Dos.version=10.0".into()]);
        }
    }
    jvm.extend([
        "-Djava.library.path=${natives_directory}".into(),
        "-Dminecraft.launcher.brand=${launcher_name}".into(),
        "-Dminecraft.launcher.version=${launcher_version}".into(),
    ]);
    let game = profile
        .minecraft_arguments
        .as_ref()
        .ok_or_else(|| PlanError::Invalid("missing game arguments".into()))?;
    Ok((jvm, game.split_whitespace().map(str::to_owned).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authenticated_quick_play_preserves_arguments_and_wrapper_boundaries() {
        let command = LaunchPlan {
            class_paths: vec!["/game with spaces/a.jar".into()],
            parameters: LaunchArguments {
                auth_access_token: "secret".into(),
                auth_uuid: "uuid".into(),
                quick_play_multiplayer: "localhost:25565".into(),
                ..Default::default()
            },
            custom_version_name: None,
            main_class: "net.fabricmc.loader.impl.launch.knot.KnotClient".into(),
            jvm_template: vec!["-cp".into(), "${classpath}".into()],
            game_template: vec![
                "--accessToken".into(),
                "${auth_access_token}".into(),
                "--quickPlayMultiplayer".into(),
                "${quickPlayMultiplayer}".into(),
            ],
            jvm: JvmOptions {
                java_major: 25,
                max_memory_mb: 4096,
                garbage_collector: GarbageCollector::Zgc,
                ..Default::default()
            },
            game: GameOptions {
                extra: "--name 'two words'".into(),
                ..Default::default()
            },
        }
        .render()
        .unwrap();
        assert!(!command.args.contains(&"-cp".into()));
        assert!(command.args.contains(&"secret".into()));
        assert!(command.args.contains(&"localhost:25565".into()));
        assert!(command.args.contains(&"two words".into()));
        assert!(command.args.contains(&"-XX:+UseZGC".into()));
        assert!(
            !export_full_launch_command(&command.class_paths, &command.args, "java")
                .contains("secret")
        );
        let spec = command
            .process_spec(
                "/java path/java",
                "/game".into(),
                HashMap::new(),
                "wrapper --flag",
            )
            .unwrap();
        assert_eq!(spec.command, "wrapper");
        assert_eq!(&spec.args[..2], &["--flag", "/java path/java"]);
        assert_eq!(spec.env["CLASSPATH"], "/game with spaces/a.jar");
    }
    #[test]
    fn unknown_placeholders_do_not_partially_replace_an_argument() {
        let values = HashMap::from([("known".into(), "${injected}".into())]);
        assert_eq!(
            replace_arguments(
                vec!["${known}/${missing}".into(), "${known}".into()],
                &values
            ),
            ["${known}/${missing}", "${injected}"]
        );
    }
}
