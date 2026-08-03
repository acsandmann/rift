use serde::{Deserialize, Serialize};

use crate::Direction;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkspaceSelector {
    Index(usize),
    Name(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreScope {
    Workspace,
    Space,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreSource {
    #[default]
    SavedActiveSpace,
    CurrentSpace,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DisplaySelector {
    Direction(Direction),
    Index(usize),
    Uuid(String),
}
