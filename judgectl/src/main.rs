use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context;
use clap::Clap;
use judge_apis::{
    live::LiveJudgeStatus,
    rest::{ByteString, JudgeJob, JudgeRequest},
};

/// Command-line JJS judge client
#[derive(Clap)]
struct Args {
    /// Name of the toolchain to use
    #[clap(long, short = 't')]
    toolchain: String,
    /// Name of the problem to use
    #[clap(long, short = 'p')]
    problem: String,
    /// Path to run source file
    #[clap(long, short = 's')]
    source: PathBuf,
    /// Judge API endpoing, e.g. http://localhost:1789
    #[clap(long, short = 'j')]
    judge_api: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Args = Clap::parse();
    let annotations = {
        let mut a = HashMap::new();
        a.insert("jjs.io/created-by".to_string(), "judgectl".to_string());
        a
    };
    let source = tokio::fs::read(&args.source)
        .await
        .context("failed to read run source")?;
    let req = JudgeRequest {
        annotations,
        toolchain_name: args.toolchain.clone(),
        problem_id: args.problem.clone(),
        run_source: ByteString(source),
    };
    let client = reqwest::Client::new();
    let result: JudgeJob = client
        .post(format!("{}/jobs", args.judge_api))
        .json(&req)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    println!("Submitted, judge job id: {}", result.id.to_hyphenated());
    let mut received_logs = HashSet::<String>::new();
    let mut printer = ProgressPrinter::new();
    loop {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let job: JudgeJob = client
            .get(format!(
                "{}/jobs/{}",
                args.judge_api,
                result.id.to_hyphenated()
            ))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        printer.add(&job.live);
        for log in job.logs {
            if received_logs.insert(log.clone()) {
                println!("New log was created: {}", log);
                let log_data = client
                    .get(format!(
                        "{}/jobs/{}/logs/{}",
                        args.judge_api,
                        job.id.to_hyphenated(),
                        log
                    ))
                    .send()
                    .await?
                    .error_for_status()?
                    .text()
                    .await?;
                let path = format!("log-{}.json", log);
                let path = Path::new(&path);
                tokio::fs::write(path, log_data)
                    .await
                    .context("failed to write log")?;
            }
        }
        if job.completed {
            println!("Completed");
            if let Some(msg) = job.error {
                anyhow::bail!("job was not successful: {}", msg);
            }
            break;
        }
    }
    Ok(())
}

struct ProgressPrinter {
    last_test: Option<u32>,
    last_score: Option<u32>,
}

impl ProgressPrinter {
    fn new() -> Self {
        ProgressPrinter {
            last_test: None,
            last_score: None,
        }
    }

    fn add(&mut self, live_status: &LiveJudgeStatus) {
        if let Some(t) = live_status.test {
            if Some(t) != self.last_test {
                self.last_test = Some(t);
                println!("Running on test {}", t);
            }
        }
        if let Some(s) = live_status.score {
            if Some(s) != self.last_score {
                self.last_score = Some(s);
                println!("Current score: {}", s);
            }
        }
    }
}
