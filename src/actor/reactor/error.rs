use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReactorError {
    #[error("Window not found: {0:?}")]
    WindowNotFound(crate::actor::app::WindowId),
    #[error("App communication failed: {0}")]
    AppCommunicationFailed(#[from] tokio::sync::mpsc::error::SendError<crate::actor::app::Request>),
    #[error("Stack line communication failed: {0}")]
    StackLineCommunicationFailed(
        #[from] tokio::sync::mpsc::error::TrySendError<crate::actor::stack_line::Event>,
    ),
    #[error("Raise manager communication failed: {0}")]
    RaiseManagerCommunicationFailed(
        #[from] tokio::sync::mpsc::error::SendError<crate::actor::raise_manager::Event>,
    ),
    #[error("Layout engine error: {0}")]
    LayoutError(String),
}
