mod rest;

use anyhow::Context;
use clap::Clap;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

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
    /// Directory containing judging logs. Set to `/dev/null` to disable logging
    #[clap(long, default_value = "/var/log/judges")]
    logs: PathBuf,
    /// Enable fake mode.
    /// In this mode judge never loads problems or toolchains and just
    /// generates random data for requests
    #[clap(long)]
    fake: bool,
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

async fn initialize_normal(args: &Args) -> anyhow::Result<rest::ServeKind> {
    let clients = create_clients(&args)
        .await
        .context("failed to initialize dependency clients")?;
    let settings = {
        let checker_logs = match &args.logs {
            p if p == Path::new("/dev/null") => (None),
            p => Some(p.join("checkers")),
        };
        if let Some(p) = &checker_logs {
            tokio::fs::create_dir_all(&p).await.with_context(|| {
                format!(
                    "failed to create directory for checker logs {}",
                    p.display()
                )
            })?;
        }
        processor::Settings { checker_logs }
    };
    Ok(rest::ServeKind::Normal { settings, clients })
}

fn initialize_fake() -> rest::ServeKind {
    rest::ServeKind::Fake {
        settings: processor::fake::FakeSettings {},
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args: Args = Clap::parse();
    tracing::info!("Running REST API");
    let cfg = rest::RestConfig { port: args.port };

    let serve_config = if args.fake {
        initialize_fake()
    } else {
        initialize_normal(&args).await?
    };

    rest::serve(cfg, serve_config).await?;
    Ok(())
}
