mod app;
mod server;
mod terminal_server;

pub use app::{
    App, EventSubscription, ManagedSession, SessionBackend, SessionBackendFuture, UserNotifier,
};
pub use server::run;

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error(transparent)]
    Runtime(#[from] lazoo_runtime::RuntimeError),
    #[error(transparent)]
    Store(#[from] lazoo_store::StoreError),
    #[error(transparent)]
    Ipc(#[from] lazoo_ipc::IpcError),
}
