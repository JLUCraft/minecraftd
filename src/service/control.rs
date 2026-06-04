use tokio::sync::oneshot;

/// Control messages sent to the instance supervisor.
#[derive(Debug)]
pub enum ControlMsg {
    /// Request to start the instance.
    Start,
    /// Request to stop the instance gracefully.
    Stop,
    /// Request to kill the instance immediately.
    Kill,
    /// Request to restart the instance.
    Restart,
    /// Send a command to the instance's stdin.
    SendCommand(String),
    /// Query the total play time in seconds.
    GetPlayTime(oneshot::Sender<u64>),
    /// Query recent output lines.
    GetRecentOutput {
        n: usize,
        tx: oneshot::Sender<Vec<String>>,
    },
    /// Shutdown the supervisor loop.
    Shutdown,
}
