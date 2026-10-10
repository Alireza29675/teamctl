//! YAML schema for `team-compose.yaml` and `projects/<id>.yaml`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

mod global;
mod load;
mod project;
mod version;

pub use global::*;
pub use load::{AgentHandle, Compose};
pub use project::*;
pub use version::SchemaVersion;

#[cfg(test)]
mod tests;
