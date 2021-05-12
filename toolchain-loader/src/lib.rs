//! This module is responsible for toolchain loading
use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

/// Toolchain description
pub struct Toolchain {
    /// Manifest
    pub spec: ToolchainSpec,
    /// Image containing toolchain files
    pub image: String,
}

/// `manifest.yaml` representation
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolchainSpec {
    /// Human-readable
    pub title: String,

    /// Machine-readable
    pub name: String,

    pub filename: String,

    #[serde(rename = "build")]
    pub build_commands: Vec<Command>,

    #[serde(rename = "run")]
    pub run_command: Command,

    #[serde(rename = "build-limits", default)]
    pub limits: pom::Limits,

    #[serde(rename = "env", default)]
    pub env: HashMap<String, String>,
}

#[derive(serde::Serialize, serde::Deserialize, Default, Debug, Clone)]
pub struct Command {
    #[serde(default = "Command::default_env")]
    pub env: HashMap<String, String>,
    pub argv: Vec<String>,
    #[serde(default = "Command::default_cwd")]
    pub cwd: String,
}

impl Command {
    fn default_env() -> HashMap<String, String> {
        HashMap::new()
    }

    fn default_cwd() -> String {
        String::from("/jjs")
    }
}

/// Responsible for fetching toolchains
pub struct ToolchainLoader {
    /// Directory containing toolchain definitions
    toolchains_dir: PathBuf,
}

impl ToolchainLoader {
    pub async fn new(toolchains_dir: &Path) -> anyhow::Result<ToolchainLoader> {
        Ok(ToolchainLoader {
            toolchains_dir: toolchains_dir.to_path_buf(),
        })
    }

    #[tracing::instrument(skip(self))]
    pub async fn resolve(&self, toolchain_name: &str) -> anyhow::Result<Toolchain> {
        let toolchain_dir_path = self.toolchains_dir.join(toolchain_name);

        let toolchain_spec = tokio::fs::read(toolchain_dir_path.join("manifest.yaml"))
            .await
            .context("toolchain config file (manifest.yaml in image root) missing")?;
        let spec: ToolchainSpec =
            serde_yaml::from_slice(&toolchain_spec).context("invalid toolchain spec")?;
        let image = tokio::fs::read_to_string(toolchain_dir_path.join("image.txt")).await?;
        let image = image.trim().to_string();
        Ok(Toolchain { spec, image })
    }
}
