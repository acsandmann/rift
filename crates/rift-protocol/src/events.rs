use std::fmt;

use serde::{Deserialize, Serialize};

/// Events available through the Mach subscription API.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    WorkspaceChanged,
    WindowsChanged,
    WindowTitleChanged,
    FocusedWindowChanged,
    StacksChanged,
    #[serde(rename = "*")]
    All,
}

impl EventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceChanged => "workspace_changed",
            Self::WindowsChanged => "windows_changed",
            Self::WindowTitleChanged => "window_title_changed",
            Self::FocusedWindowChanged => "focused_window_changed",
            Self::StacksChanged => "stacks_changed",
            Self::All => "*",
        }
    }
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(self.as_str()) }
}
