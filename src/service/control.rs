use tokio::sync::oneshot;

/// Control messages sent to the instance supervisor.
#[derive(Debug)]
pub enum ControlMsg {
    /// Execute an action and report its actual outcome, not merely queue admission.
    Acknowledged {
        command: Box<Self>,
        reply: oneshot::Sender<Result<(), crate::instance::base::InstanceError>>,
    },
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
    /// Query the operating-system process ID while the instance is running.
    GetProcessId(oneshot::Sender<Option<u32>>),
    /// Query recent output lines.
    GetRecentOutput {
        n: usize,
        tx: oneshot::Sender<Vec<String>>,
    },
    /// Query recent output lines and the first global line index.
    GetRecentOutputWithOffset {
        n: usize,
        tx: oneshot::Sender<(u64, Vec<String>)>,
    },
    /// Shutdown the supervisor loop.
    Shutdown,
}
