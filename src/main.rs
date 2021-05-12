mod rest;

use anyhow::Context;
use clap::Clap;
use std::{path::PathBuf, sync::Arc};

#[derive(Clap)]
struct Args {
    /// Port that judge should listen
    #[clap(long, default_value = "1789")]
    port: u16,
    /// Address which can be used to connect to invoker
    #[clap(long)]
    invoker: String,
    /// Directory containing toolchain manifests
    #[clap(long)]
    toolchains: PathBuf,
    /// Directory for caching loaded problems
    #[clap(long, default_value = "/tmp/jjs-judge-problems-cache")]
    problems_cache: PathBuf,
    /// Directory containing locally available problems
    #[clap(long)]
    problems_source_dir: Option<PathBuf>,
    /// URL identifying MongoDB database containing problems
    #[clap(long)]
    problems_source_mongodb: Option<String>,
}

async fn create_clients(args: &Args) -> anyhow::Result<processor::Clients> {
    let mut invokers = invoker_client::Client::builder();
    invokers.add(invoker_client::Pool::new_from_address(&args.invoker));
    let toolchains = toolchain_loader::ToolchainLoader::new(&args.toolchains)
        .await
        .context("failed to initialize toolchain loader")?;
    let problem_loader_config = problem_loader::LoaderConfig {
        fs: args.problems_source_dir.clone(),
        mongodb: args.problems_source_mongodb.clone(),
    };
    let problems =
        problem_loader::Loader::from_config(&problem_loader_config, args.problems_cache.clone())
            .await
            .context("failed to initialize problem loader")?;

    Ok(processor::Clients {
        invokers: invokers.build(),
        toolchains: Arc::new(toolchains),
        problems: Arc::new(problems),
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args: Args = Clap::parse();
    let clients = create_clients(&args)
        .await
        .context("failed to initialize dependency clients")?;
    tracing::info!("Running REST API");
    let cfg = rest::RestConfig { port: args.port };
    rest::serve(cfg, clients).await?;
    Ok(())
}
