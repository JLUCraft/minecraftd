#![deny(warnings)]
#![warn(single_use_lifetimes)]
#![warn(clippy::nursery)]
#![warn(clippy::unwrap_used)]
#![warn(clippy::panic)]
#![warn(clippy::todo)]

use clap::{Parser, Subcommand};
use interprocess::local_socket::tokio::Stream;
use interprocess::local_socket::traits::tokio::Stream as _;
use std::io::Write;
use std::path::PathBuf;
use std::process;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use minecraftd::daemon::{self, CliRequest, CliResponse};
use minecraftd::instance::path::InstanceSubdir;

/// Minecraft instance manager.
#[derive(Parser)]
#[command(name = "minecraftd", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the background daemon
    Daemon,
    /// Create and start a new Minecraft server instance
    Run {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long, default_value_t = false)]
        restart: bool,
    },
    /// List all discovered instances (local scan + daemon running state)
    Ps,
    /// Stop a running instance gracefully
    Stop { id: String },
    /// Kill an instance immediately
    Kill { id: String },
    /// Show recent stdout/stderr
    Logs {
        id: String,
        #[arg(short, long, default_value = "50")]
        tail: usize,
    },
    /// Send a command to instance stdin
    Exec { id: String, command: Vec<String> },
    /// Show running instance details
    Info { id: String },
    /// Manage per-instance backups (local, no daemon needed)
    #[command(subcommand)]
    Archive(ArchiveCmd),
}

#[derive(Subcommand)]
enum ArchiveCmd {
    /// List all backups for an instance
    List { id: String },
    /// Create a backup of an instance's world data
    Create {
        id: String,
        #[arg(short, long)]
        label: String,
    },
    /// Restore a backup to the instance directory
    Restore { id: String, backup_name: String },
    /// Delete a backup
    Delete { id: String, backup_name: String },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "minecraftd=info".into()))
        .init();

    let cli = Cli::parse();

    match &cli.command {
        Commands::Daemon => {
            run_daemon().await;
        }
        Commands::Ps => {
            cmd_ps().await;
        }
        Commands::Archive(cmd) => {
            cmd_archive(cmd);
        }
        Commands::Info { id } => {
            cmd_info(id).await;
        }
        _ => {
            let result = send_to_daemon(&cli.command).await;
            match result {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("error: {e}");
                    process::exit(1);
                }
            }
        }
    }
}

// -----------------------------------------------------------------------
// Daemon
// -----------------------------------------------------------------------

async fn run_daemon() {
    let daemon = daemon::Daemon::new();
    daemon.run().await;
}

/// Try to connect to the daemon. Returns Ok(stream) or the error.
async fn connect() -> Result<Stream, String> {
    let name = daemon::socket_name();
    Stream::connect(name)
        .await
        .map_err(|e| format!("connect to daemon: {e}"))
}

async fn send_to_daemon(command: &Commands) -> Result<(), String> {
    let request = command_to_request(command)?;
    let json = serde_json::to_string(&request).map_err(|e| format!("serialize: {e}"))?;

    let (reader, mut writer) = connect().await?.split();

    writer
        .write_all(json.as_bytes())
        .await
        .map_err(|e| format!("write: {e}"))?;
    writer
        .write_all(b"\n")
        .await
        .map_err(|e| format!("write: {e}"))?;
    drop(writer);

    let mut buf = String::new();
    let mut buf_reader = BufReader::new(reader);
    buf_reader
        .read_line(&mut buf)
        .await
        .map_err(|e| format!("read: {e}"))?;

    let response: CliResponse =
        serde_json::from_str(buf.trim()).map_err(|e| format!("deserialize: {e}"))?;

    print_response(&response);
    Ok(())
}

fn command_to_request(command: &Commands) -> Result<CliRequest, String> {
    match command {
        Commands::Run { config, restart } => Ok(CliRequest::Run {
            config_path: config.clone(),
            restart: *restart,
        }),
        Commands::Stop { id } => Ok(CliRequest::Stop { id: id.clone() }),
        Commands::Kill { id } => Ok(CliRequest::Kill { id: id.clone() }),
        Commands::Logs { id, tail } => Ok(CliRequest::Logs {
            id: id.clone(),
            tail: *tail,
        }),
        Commands::Exec { id, command } => {
            if command.is_empty() {
                return Err("exec requires a command".into());
            }
            Ok(CliRequest::Exec {
                id: id.clone(),
                command: command.join(" "),
            })
        }
        _ => Err("command must be sent to daemon".into()),
    }
}

fn print_response(response: &CliResponse) {
    match response {
        CliResponse::Ok { message: Some(msg) } => {
            println!("{msg}");
        }
        CliResponse::Ok { .. } => {}
        CliResponse::Logs { lines } => {
            for line in lines {
                println!("{line}");
            }
        }
        CliResponse::Err { error } => {
            eprintln!("error: {error}");
        }
        _ => {}
    }
}

// -----------------------------------------------------------------------
// ps: local discovery + daemon running state
// -----------------------------------------------------------------------

async fn cmd_ps() {
    use minecraftd::discovery;

    let discovered = discovery::discover();
    let running_ids = query_running().await;

    if discovered.is_empty() {
        println!("No Minecraft instances found.");
        let dirs: Vec<String> = discovery::known_dirs()
            .iter()
            .map(|d| d.display().to_string())
            .collect();
        println!("Scanned:\n  {}", dirs.join("\n  "));
        if !running_ids.is_empty() {
            println!("(daemon reports {} running)", running_ids.len());
        }
        return;
    }

    println!("{:<30} {:<12} {:<14} PATH", "ID", "STATE", "KIND");
    for inst in &discovered {
        let state = if running_ids.contains(&inst.id) {
            "Running"
        } else {
            "Stopped"
        };
        println!(
            "{:<30} {:<12} {:<14} {}",
            inst.id,
            state,
            inst.kind.as_str(),
            inst.path.display()
        );
    }
}

async fn query_running() -> Vec<String> {
    let request = CliRequest::Running;
    let json = match serde_json::to_string(&request) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let (reader, mut writer) = match connect().await {
        Ok(s) => s.split(),
        Err(_) => return Vec::new(),
    };

    if writer.write_all(json.as_bytes()).await.is_err() || writer.write_all(b"\n").await.is_err() {
        return Vec::new();
    }
    drop(writer);

    let mut buf = String::new();
    let mut buf_reader = BufReader::new(reader);
    if buf_reader.read_line(&mut buf).await.is_err() {
        return Vec::new();
    }

    match serde_json::from_str::<CliResponse>(buf.trim()) {
        Ok(CliResponse::RunningIds { ids }) => ids,
        _ => Vec::new(),
    }
}

// -----------------------------------------------------------------------
// info
// -----------------------------------------------------------------------

async fn cmd_info(id: &str) {
    let discovered = minecraftd::discovery::discover();
    let running_ids = query_running().await;

    match discovered.iter().find(|i| i.id == id) {
        Some(inst) => {
            let state = if running_ids.contains(&inst.id) {
                "Running"
            } else {
                "Stopped"
            };
            println!("ID:      {}", inst.id);
            println!("Path:    {}", inst.path.display());
            println!("State:   {state}");
            println!("Kind:    {}", inst.kind.as_str());
            println!("Worlds:  {}", inst.worlds.join(", "));
        }
        None => {
            eprintln!("error: instance '{id}' not found on disk");
            process::exit(1);
        }
    }
}

// -----------------------------------------------------------------------
// Archive: local-only, per-instance
// -----------------------------------------------------------------------

fn cmd_archive(cmd: &ArchiveCmd) {
    match cmd {
        ArchiveCmd::List { id } => {
            let Some(inst) = find_instance(id) else {
                eprintln!("error: instance '{id}' not found");
                process::exit(1);
            };
            let backup_dir = inst.path.join(InstanceSubdir::Backups.dir_name());
            if !backup_dir.is_dir() {
                println!("No backups for {id}.");
                return;
            }

            let mut entries: Vec<String> = match std::fs::read_dir(&backup_dir) {
                Ok(read) => read
                    .flatten()
                    .filter_map(|e| {
                        e.path()
                            .file_name()
                            .and_then(|n| n.to_str())
                            .map(String::from)
                    })
                    .collect(),
                Err(e) => {
                    eprintln!("error: cannot read backup dir: {e}");
                    process::exit(1);
                }
            };
            entries.sort();

            if entries.is_empty() {
                println!("No backups for {id}.");
                return;
            }
            println!("Backups for {id}:");
            for e in &entries {
                println!("  {e}");
            }
        }
        ArchiveCmd::Create { id, label } => {
            let Some(inst) = find_instance(id) else {
                eprintln!("error: instance '{id}' not found");
                process::exit(1);
            };
            let backup_dir = inst.path.join(InstanceSubdir::Backups.dir_name());
            std::fs::create_dir_all(&backup_dir).unwrap_or_else(|e| {
                eprintln!("error: cannot create backup dir: {e}");
                process::exit(1);
            });

            let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
            let safe_label = label.replace([' ', '/'], "_");
            let archive_name = format!("{timestamp}_{safe_label}.zip");
            let archive_path = backup_dir.join(&archive_name);

            match create_backup_zip(&inst.path, &archive_path) {
                Ok(()) => println!("backup created: {archive_name}"),
                Err(e) => {
                    eprintln!("error: backup failed: {e}");
                    process::exit(1);
                }
            }
        }
        ArchiveCmd::Restore { id, backup_name } => {
            let Some(inst) = find_instance(id) else {
                eprintln!("error: instance '{id}' not found");
                process::exit(1);
            };
            let archive_path = inst
                .path
                .join(InstanceSubdir::Backups.dir_name())
                .join(backup_name);
            if !archive_path.is_file() {
                eprintln!("error: backup '{backup_name}' not found");
                process::exit(1);
            }
            match restore_backup_zip(&archive_path, &inst.path) {
                Ok(()) => println!("backup '{backup_name}' restored to {}", inst.path.display()),
                Err(e) => {
                    eprintln!("error: restore failed: {e}");
                    process::exit(1);
                }
            }
        }
        ArchiveCmd::Delete { id, backup_name } => {
            let Some(inst) = find_instance(id) else {
                eprintln!("error: instance '{id}' not found");
                process::exit(1);
            };
            let archive_path = inst
                .path
                .join(InstanceSubdir::Backups.dir_name())
                .join(backup_name);
            if !archive_path.is_file() {
                eprintln!("error: backup '{backup_name}' not found");
                process::exit(1);
            }
            match std::fs::remove_file(&archive_path) {
                Ok(()) => println!("backup '{backup_name}' deleted"),
                Err(e) => {
                    eprintln!("error: cannot delete: {e}");
                    process::exit(1);
                }
            }
        }
    }
}

fn find_instance(id: &str) -> Option<minecraftd::discovery::DiscoveredInstance> {
    minecraftd::discovery::discover()
        .into_iter()
        .find(|i| i.id == id)
}

/// Create a zip backup of the instance directory.
fn create_backup_zip(
    instance_dir: &std::path::Path,
    archive_path: &std::path::Path,
) -> Result<(), String> {
    let file = std::fs::File::create(archive_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    for entry in walkdir::WalkDir::new(instance_dir) {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // Skip backups directory to avoid recursive backup
        if path
            .strip_prefix(instance_dir)
            .map_err(|e| e.to_string())?
            .components()
            .next()
            .is_some_and(|c| c.as_os_str() == InstanceSubdir::Backups.dir_name())
        {
            continue;
        }
        let rel = path.strip_prefix(instance_dir).map_err(|e| e.to_string())?;
        let name = rel.display().to_string();
        zip.start_file(name, options).map_err(|e| e.to_string())?;
        let data = std::fs::read(path).map_err(|e| e.to_string())?;
        zip.write_all(&data).map_err(|e| e.to_string())?;
    }

    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

/// Restore a zip backup into the instance directory.
fn restore_backup_zip(
    archive_path: &std::path::Path,
    target_dir: &std::path::Path,
) -> Result<(), String> {
    let file = std::fs::File::open(archive_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().to_string();

        // Safety: reject path traversal
        if name.contains("..") {
            continue;
        }

        let dest = target_dir.join(&name);

        if entry.is_dir() {
            std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut out = std::fs::File::create(&dest).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}
