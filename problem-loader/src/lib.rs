//! This library is responsible for fetching problem packages

mod registry;

use anyhow::Context;
use registry::Registry;
use std::{collections::HashMap, path::PathBuf};

// TODO: cache expiration, checksum, etc
/// Stores cached problem information
struct ProblemCache {
    /// Maps problem name to problem cache.
    items: HashMap<String, ProblemCacheItem>,
}

impl ProblemCache {
    fn new() -> ProblemCache {
        ProblemCache {
            items: HashMap::new(),
        }
    }
}

struct ProblemCacheItem {
    assets: PathBuf,
    manifest: pom::Problem,
}

pub struct Loader {
    registries: Vec<Box<dyn Registry>>,
    cache: tokio::sync::Mutex<ProblemCache>,
    /// Each problem will be represented by ${cache_dir}/${problem_name}
    cache_dir: PathBuf,
}

impl Loader {
    pub async fn from_config(conf: &LoaderConfig, cache_dir: PathBuf) -> anyhow::Result<Loader> {
        tokio::fs::create_dir_all(&cache_dir)
            .await
            .with_context(|| format!("failed to create problems dir at {}", cache_dir.display()))?;
        let mut loader = Loader {
            registries: vec![],
            cache_dir,
            cache: tokio::sync::Mutex::new(ProblemCache::new()),
        };
        if let Some(fs) = &conf.fs {
            let fs_reg = registry::FsRegistry::new(fs.clone());
            loader.registries.push(Box::new(fs_reg));
        }
        if let Some(mongodb) = &conf.mongodb {
            let mongo_reg = registry::MongoRegistry::new(mongodb)
                .await
                .context("unable to initialize MongodbRegistry")?;
            loader.registries.push(Box::new(mongo_reg));
        }
        Ok(loader)
    }

    /// Tries to resolve problem named `problem_name` in all configured
    /// registries. On success, returns problem manifest and path to assets dir.
    #[tracing::instrument(skip(self))]
    pub async fn find(
        &self,
        problem_name: &str,
    ) -> anyhow::Result<Option<(pom::Problem, PathBuf)>> {
        let mut cache = self.cache.lock().await;
        if let Some(cached_info) = cache.items.get(problem_name) {
            tracing::info!("Found problem in cache");
            return Ok(Some((
                cached_info.manifest.clone(),
                cached_info.assets.clone(),
            )));
        }
        tracing::info!("cache miss");
        // cache for this problem not found, let's load it.
        let assets_path = self.cache_dir.join(problem_name);
        tokio::fs::remove_dir_all(&assets_path).await.ok();
        tokio::fs::create_dir(&assets_path).await.with_context(|| {
            format!(
                "failed to prepare problem assets directory at {}",
                assets_path.display()
            )
        })?;
        for registry in &self.registries {
            let res = registry
                .get_problem(problem_name, &assets_path)
                .await
                .with_context(|| {
                    format!(
                        "failed to search for problem {} in registry {}",
                        problem_name,
                        registry.name()
                    )
                })?;

            if let Some(manifest) = res {
                tracing::info!(
                    registry_name = registry.name(),
                    "successfully resolved problem"
                );
                cache.items.insert(
                    problem_name.to_string(),
                    ProblemCacheItem {
                        manifest: manifest.clone(),
                        assets: assets_path.clone(),
                    },
                );
                return Ok(Some((manifest, assets_path)));
            }
        }
        // no registry knows about this problem
        tracing::warn!("problem not found");
        Ok(None)
    }
}

/// Used in [`from_config`](Loader::from_config) constructor
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LoaderConfig {
    #[serde(default)]
    pub fs: Option<std::path::PathBuf>,
    #[serde(default)]
    pub mongodb: Option<String>,
}
