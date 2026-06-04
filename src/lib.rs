#![deny(warnings)]
#![warn(single_use_lifetimes)]
#![warn(clippy::nursery)]
#![warn(clippy::unwrap_used)]
#![warn(clippy::panic)]
#![warn(clippy::todo)]

pub mod archive;
pub mod config;
pub mod core;
pub mod daemon;
pub mod discovery;
pub mod download;
pub mod instance;
pub mod lifecycle;
pub mod minecraft;
pub mod modpack;
pub mod platform;
pub mod process;
pub mod resource;
pub mod service;
pub mod task;
pub mod util;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("instance error: {0}")]
    Instance(#[from] instance::base::InstanceError),
    #[error("state error: {0}")]
    State(#[from] core::state::StateError),
    #[error("config error: {0}")]
    Config(#[from] core::config::ConfigError),
    #[error("process error: {0}")]
    Process(#[from] core::process::ProcessError),
    #[error("spawn error: {0}")]
    Spawn(#[from] core::process::SpawnError),
    #[error("task error: {0}")]
    Task(#[from] task::scheduler::TaskError),
}

pub type Result<T> = std::result::Result<T, Error>;
