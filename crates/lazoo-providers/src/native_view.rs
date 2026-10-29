//! Provider-neutral ownership for native read-only views.
//!
//! A provider supplies a small backend which owns its restricted remote bridge or signals that
//! the native process is already represented by the daemon's canonical terminal.  The lease is
//! intentionally not serializable: launch overlays can contain bearer capabilities.

use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use lazoo_runtime::{TerminalSession, TerminalView};
use tokio::sync::watch;

use crate::CommandSpec;

/// Private terminal-launch data. It deliberately has no serialization implementation.
pub enum NativeViewLaunchDescriptor {
    /// A restricted remote native viewer that needs its own terminal process.
    Remote {
        remote_url: String,
        command: CommandSpec,
    },
    /// The native process is the provider's canonical terminal and is borrowed read-only.
    ExistingTerminal {
        terminal: TerminalView,
        command: CommandSpec,
    },
}

impl NativeViewLaunchDescriptor {
    #[must_use]
    pub fn remote_url(&self) -> Option<&str> {
        match self {
            Self::Remote { remote_url, .. } => Some(remote_url),
            Self::ExistingTerminal { .. } => None,
        }
    }

    /// Caller must keep this in an internal terminal environment overlay.
    #[must_use]
    pub fn command(&self) -> CommandSpec {
        match self {
            Self::Remote { command, .. } | Self::ExistingTerminal { command, .. } => {
                command.clone()
            }
        }
    }

    #[must_use]
    pub fn existing_terminal(&self) -> Option<TerminalView> {
        match self {
            Self::ExistingTerminal { terminal, .. } => Some(terminal.clone()),
            Self::Remote { .. } => None,
        }
    }
}

impl fmt::Debug for NativeViewLaunchDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Remote {
                remote_url,
                command,
            } => f
                .debug_struct("NativeViewLaunchDescriptor")
                .field("remote_url", remote_url)
                .field("executable", &command.executable)
                .field("args", &command.args)
                .field("cwd", &command.cwd)
                .field("env_keys", &command.env.keys().collect::<Vec<_>>())
                .finish_non_exhaustive(),
            Self::ExistingTerminal { command, .. } => f
                .debug_struct("NativeViewLaunchDescriptor")
                .field("terminal", &"existing")
                .field("executable", &command.executable)
                .field("args", &command.args)
                .field("cwd", &command.cwd)
                .field("env_keys", &command.env.keys().collect::<Vec<_>>())
                .finish(),
        }
    }
}

/// Provider-private lease mechanics.  This trait is deliberately crate-private so adapters, not
/// callers, decide whether a view is remote or borrowed and retain authority over its closure.
pub(crate) trait NativeViewBackend: Send + Sync {
    fn close(&self);
    fn is_closed(&self) -> bool;
    fn closed_subscription(&self) -> watch::Receiver<bool>;
    fn launch_descriptor(&self) -> NativeViewLaunchDescriptor;
}

/// Owns a provider-native, read-only view capability.
pub struct NativeViewLease {
    backend: Arc<dyn NativeViewBackend>,
}

impl fmt::Debug for NativeViewLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeViewLease")
            .field("descriptor", &self.launch_descriptor())
            .field("closed", &self.is_closed())
            .finish()
    }
}

impl NativeViewLease {
    pub(crate) fn from_backend(backend: impl NativeViewBackend + 'static) -> Self {
        Self {
            backend: Arc::new(backend),
        }
    }

    /// Creates a read-only view over a provider-owned canonical terminal. Closing this lease
    /// only drops the logical view. A lightweight monitor closes the lease when that terminal
    /// exits, without stopping or otherwise taking ownership of the process tree.
    #[must_use]
    pub(crate) fn borrow_terminal(terminal: &TerminalSession, command: CommandSpec) -> Self {
        Self::from_backend(BorrowedTerminalBackend::new(terminal.view(), command))
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.backend.is_closed()
    }

    #[must_use]
    pub fn closed_subscription(&self) -> watch::Receiver<bool> {
        self.backend.closed_subscription()
    }

    /// Closing a view has no authority to cancel canonical provider work.
    pub fn close(&self) {
        self.backend.close();
    }

    #[must_use]
    pub fn launch_descriptor(&self) -> NativeViewLaunchDescriptor {
        self.backend.launch_descriptor()
    }
}

struct BorrowedTerminalBackend {
    terminal: TerminalView,
    command: CommandSpec,
    closed: Arc<AtomicBool>,
    closed_tx: watch::Sender<bool>,
}

impl BorrowedTerminalBackend {
    fn new(terminal: TerminalView, command: CommandSpec) -> Self {
        let closed = Arc::new(AtomicBool::new(false));
        let (closed_tx, mut closed_rx) = watch::channel(false);
        let watched_terminal = terminal.clone();
        let watched_closed = Arc::clone(&closed);
        let watched_tx = closed_tx.clone();
        tokio::spawn(async move {
            loop {
                if *closed_rx.borrow_and_update()
                    || !watched_terminal
                        .exit_status()
                        .map_or(true, |status| status.live)
                {
                    if !watched_closed.swap(true, Ordering::AcqRel) {
                        watched_tx.send_replace(true);
                    }
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
        Self {
            terminal,
            command,
            closed,
            closed_tx,
        }
    }
}

impl NativeViewBackend for BorrowedTerminalBackend {
    fn close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.closed_tx.send_replace(true);
        }
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn closed_subscription(&self) -> watch::Receiver<bool> {
        self.closed_tx.subscribe()
    }

    fn launch_descriptor(&self) -> NativeViewLaunchDescriptor {
        NativeViewLaunchDescriptor::ExistingTerminal {
            terminal: self.terminal.clone(),
            command: self.command.clone(),
        }
    }
}

impl Drop for NativeViewLease {
    fn drop(&mut self) {
        self.close();
    }
}
