//! Processor is part of judge that deals with a single run (and it doesn't
//! care where have it come from).

mod compile;
mod exec_test;
mod request_builder;
mod transform_judge_log;

use anyhow::Context;
use invoker_api::invoke::{CommandResult, Limits};
use judge_apis::judge_log::JudgeLog;
use pom::Valuer;
use std::{
    borrow::Cow,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::{mpsc, oneshot};
use tracing::Instrument;
use valuer_api::{
    status_codes, JudgeLogKind, ProblemInfo, Status, StatusKind, TestDoneNotification,
    ValuerResponse,
};
use valuer_client::{ChildClientConfig, ClientConfig};

/// Single judging request
pub struct Request {
    /// Toolchain name (will be passed to toolchain loader)
    pub toolchain_name: String,
    /// Problem name (will be passed to problem loader)
    pub problem_id: String,
    /// Run source
    pub run_source: Vec<u8>,
}

/// Part of response stream
pub enum Event {
    /// A judge log has been created.
    /// Sent at most once per each judge log king.
    LogCreated(judge_apis::judge_log::JudgeLog),
    /// Live status update: run is being judged on given test.
    LiveTest(u32),
    /// Live status update: run has reached given score.
    LiveScore(u32),
}

/// Overall response state
#[derive(Debug)]
pub enum JudgeOutcome {
    /// Run was judged successfully, so all reported information
    /// is OK.
    /// All protocols were sent already.
    Success,
    /// Run was not judged, because of internal error.
    /// Maybe several protocols were emitted, but results are neither precise nor complete
    Fault { error: anyhow::Error },
}

/// Contains invoker client, toolchain loader and problem loader
#[derive(Clone)]
pub struct Clients {
    pub toolchains: Arc<toolchain_loader::ToolchainLoader>,
    pub problems: Arc<problem_loader::Loader>,
    pub invokers: invoker_client::Client,
}

/// Settings are global rather then come from a request.
#[derive(Clone)]
pub struct Settings {
    /// ${checker_logs}/${job_id}/${test_id} will contain checker log
    /// for a test test_id.
    pub checker_logs: Option<PathBuf>,
}

/// The main function, which responds to a single request.
#[tracing::instrument(skip(req, clients, settings))]
pub fn judge(req: Request, clients: Clients, settings: Settings) -> JobProgress {
    let (done_tx, done_rx) = oneshot::channel();
    let (events_tx, events_rx) = mpsc::channel(1);
    tokio::task::spawn(
        async move {
            let mut protocol_sender = ProtocolSender {
                sent: Vec::new(),
                tx: events_tx.clone(),
                // TODO: read from request
                debug_dump_dir: None,
            };

            let res = do_judge(req, events_tx, clients, &mut protocol_sender, settings).await;
            if let Err(err) = &res {
                tracing::warn!(err = %format_args!("{:#}", err),"judging failed, responding with judge fault");
                protocol_sender
                    .send_fake_logs(
                        Status {
                            kind: StatusKind::InternalError,
                            code: status_codes::JUDGE_FAULT.to_string(),
                        },
                        "",
                    )
                    .await;
            }
            done_tx.send(res).ok();
        }
        .in_current_span(),
    );
    JobProgress { events_rx, done_rx }
}

/// Can be used to view judge job progress
pub struct JobProgress {
    events_rx: mpsc::Receiver<Event>,
    done_rx: oneshot::Receiver<anyhow::Result<()>>,
}

impl JobProgress {
    /// Wait for completion. All pending events will be dropped.
    pub async fn wait(self) -> JudgeOutcome {
        let res = self
            .done_rx
            .await
            .unwrap_or_else(|_| Err(anyhow::Error::msg("background task stopped unexpectedly")));
        match res {
            Ok(()) => JudgeOutcome::Success,
            Err(error) => JudgeOutcome::Fault { error },
        }
    }

    /// Returns next event.
    pub async fn event(&mut self) -> Option<Event> {
        self.events_rx.recv().await
    }
}

async fn do_judge(
    req: Request,
    tx: mpsc::Sender<Event>,
    clients: Clients,
    protocol_sender: &mut ProtocolSender,
    settings: Settings,
) -> anyhow::Result<()> {
    tracing::info!("loading problem");
    let (problem, problem_assets) = clients
        .problems
        .find(&req.problem_id)
        .await
        .context("failed to get problem")?
        .context("problem not found")?;

    let file_ref_resolver = FileRefResolver {
        problem_assets_dir: problem_assets.clone(),
    };

    tracing::info!("loading toolchain");
    let toolchain = clients
        .toolchains
        .resolve(&req.toolchain_name)
        .await
        .context("failed to find toolchain")?;

    tracing::info!("compiling");
    let mut compile_res = compile::compile(&req, &toolchain, clients.invokers.clone()).await?;
    let built = match &mut compile_res.result {
        Ok(b) => b.take().expect("compile does not return none"),
        Err(status) => {
            tracing::info!("compilation failed");
            protocol_sender
                .send_fake_logs(status.clone(), &compile_res.log)
                .await;
            return Ok(());
        }
    };
    let compile_res = compile_res;
    tracing::info!("running tests");

    let valuer_config = match &problem.valuer {
        Valuer::Child(child) => {
            let current_dir = match &child.current_dir {
                Some(p) => file_ref_resolver.resolve_asset(p),
                None => {
                    tracing::debug!(
                        "valuer current_directory unset in problem manifest, defaulting to problem assets directory"
                    );
                    problem_assets.clone()
                }
            };
            ClientConfig::Child(ChildClientConfig {
                exe: file_ref_resolver.resolve_asset(&child.exe),
                args: child.extra_args.clone(),
                current_dir,
            })
        }
    };
    let mut valuer = valuer_client::ValuerClient::new(&valuer_config)
        .await
        .context("failed to initialize valuer")?;
    valuer
        .write_problem_data(ProblemInfo {
            tests: problem
                .tests
                .iter()
                .map(|test_spec| test_spec.group.clone())
                .collect(),
        })
        .await
        .context("failed to send problem info to valuer")?;
    let mut test_results = Vec::new();
    loop {
        match valuer.poll().await? {
            ValuerResponse::Test { test_id: tid, live } => {
                if live {
                    tx.send(Event::LiveTest(tid.get())).await.ok();
                }

                let test_result = exec_test::exec(
                    &toolchain,
                    &problem,
                    clients.invokers.clone(),
                    &file_ref_resolver,
                    tid,
                    &settings,
                    &built,
                )
                .await
                .with_context(|| format!("failed to judge solution on test {}", tid))?;
                test_results.push((tid, test_result.clone()));
                valuer
                    .notify_test_done(TestDoneNotification {
                        test_id: tid,
                        test_status: test_result.status,
                    })
                    .await
                    .with_context(|| {
                        format!("failed to notify valuer that test {} is done", tid)
                    })?;
            }
            ValuerResponse::Finish => {
                break;
            }
            ValuerResponse::LiveScore { score } => {
                tx.send(Event::LiveScore(score)).await.ok();
            }
            ValuerResponse::JudgeLog(judge_log) => {
                let converted_judge_log = transform_judge_log::transform(
                    &judge_log,
                    &compile_res,
                    &test_results,
                    &problem,
                    &file_ref_resolver,
                )
                .await
                .context("failed to convert valuer judge log to invoker judge log")?;

                protocol_sender.send_log(converted_judge_log).await;
            }
        }
    }

    Ok(())
}

enum CommandStatus {
    /// Startup error
    Startup,
    /// Time limit exceeded
    TimeLimit,
    /// Memory limit exceeded
    MemLimit,
    /// Command has failed
    Runtime,
    /// Command has finished successfully
    Ok,
}

fn describe_command_result(limits: &Limits, data: &CommandResult) -> CommandStatus {
    if data.spawn_error.is_some() {
        return CommandStatus::Startup;
    }
    if let Some(usage) = data.cpu_time {
        if usage > limits.time * 1_000_000 {
            return CommandStatus::TimeLimit;
        }
    }
    if let Some(usage) = data.memory {
        if usage > limits.memory {
            return CommandStatus::MemLimit;
        }
    }
    if data.exit_code != 0 {
        return CommandStatus::Runtime;
    }
    CommandStatus::Ok
}

struct FileRefResolver {
    problem_assets_dir: PathBuf,
}

impl FileRefResolver {
    fn resolve_asset(&self, short_path: &pom::FileRef) -> PathBuf {
        let root: Cow<Path> = match short_path.root {
            pom::FileRefRoot::Problem => self.problem_assets_dir.clone().into(),
            pom::FileRefRoot::Root => Path::new("/").into(),
        };

        root.join(&short_path.path)
    }
}

struct ProtocolSender {
    sent: Vec<JudgeLogKind>,
    tx: mpsc::Sender<Event>,
    debug_dump_dir: Option<PathBuf>,
}

impl ProtocolSender {
    async fn send_fake_logs(&mut self, status: Status, compile_log: &str) {
        for kind in JudgeLogKind::list() {
            if self.sent.contains(&kind) {
                continue;
            }
            tracing::info!("creating fake protocol of kind {}", kind.as_str());
            let fake = JudgeLog {
                kind,
                tests: Vec::new(),
                subtasks: Vec::new(),
                compile_log: compile_log.to_string(),
                score: 0,
                is_full: false,
                status: status.clone(),
            };
            self.send_log(fake).await;
        }
    }

    #[tracing::instrument(skip(self, log), fields(log_kind = log.kind.as_str()))]
    async fn send_log(&mut self, log: JudgeLog) {
        let already_sent = self.sent.contains(&log.kind);
        if already_sent {
            panic!("bug: log of kind {} sent twice", log.kind.as_str());
        }
        self.sent.push(log.kind);
        if let Some(d) = &self.debug_dump_dir {
            let dest = d.join(log.kind.as_str());
            if let Err(e) = Self::try_put_log_to(&log, &dest).await {
                tracing::warn!("failed to save debug dump of the log: {:#}", e);
            }
        }
        self.tx.send(Event::LogCreated(log)).await.ok();
    }

    async fn try_put_log_to(log: &JudgeLog, dest: &Path) -> anyhow::Result<()> {
        let log = serde_json::to_vec_pretty(log).context("failed to serialize log")?;
        tokio::fs::write(dest, log)
            .await
            .with_context(|| format!("failed to write log to {}", dest.display()))?;
        Ok(())
    }
}
