//! Judge REST api

use anyhow::Context;
use api_util::{ApiError, ErrorKind};
use futures::future::{FutureExt, TryFutureExt};
use std::{collections::HashMap, convert::Infallible, sync::Arc};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;
use warp::Filter;

pub struct RestConfig {
    pub port: u16,
}

/// Contains information about single judge job
struct JudgeJob {
    id: Uuid,
    live_test: Option<u32>,
    live_score: Option<u32>,
    logs: HashMap<String, judge_apis::judge_log::JudgeLog>,
    annotations: HashMap<String, String>,
    outcome: Option<processor::JudgeOutcome>,
}

impl JudgeJob {
    fn as_rest(&self) -> judge_apis::rest::JudgeJob {
        let error = match &self.outcome {
            Some(processor::JudgeOutcome::Fault { error }) => Some(format!("{:#}", error)),
            _ => None,
        };
        judge_apis::rest::JudgeJob {
            id: self.id,
            logs: self.logs.keys().cloned().collect(),
            annotations: self.annotations.clone(),
            completed: self.outcome.is_some(),
            live: judge_apis::live::LiveJudgeStatus {
                test: self.live_test,
                score: self.live_score,
            },
            error,
        }
    }
}

struct State {
    judge: RwLock<HashMap<Uuid, Arc<Mutex<JudgeJob>>>>,
    clients: processor::Clients,
    settings: processor::Settings,
}

async fn start_job(
    state: Arc<State>,
    req: judge_apis::rest::JudgeRequest,
) -> judge_apis::rest::JudgeJob {
    let proc_request = processor::Request {
        toolchain_name: req.toolchain_name,
        problem_id: req.problem_id,
        run_source: req.run_source.0,
    };
    let job_id = Uuid::new_v4();
    let mut settings = state.settings.clone();
    {
        let mut job_id_s = Uuid::encode_buffer();
        let job_id_s = job_id.to_hyphenated().encode_lower(&mut job_id_s);
        if let Some(p) = &mut settings.checker_logs {
            p.push(&*job_id_s);
        }
    }
    let mut progress = processor::judge(proc_request, state.clients.clone(), settings);
    let job = JudgeJob {
        id: job_id,
        live_test: None,
        live_score: None,
        logs: HashMap::new(),
        annotations: req.annotations,
        outcome: None,
    };

    let resp = job.as_rest();

    let job = Arc::new(Mutex::new(job));
    let prev = state.judge.write().await.insert(job_id, job.clone());
    assert!(prev.is_none());
    tokio::task::spawn(async move {
        while let Some(ev) = progress.event().await {
            let mut job = job.lock().await;
            match ev {
                processor::Event::LiveScore(ls) => {
                    job.live_score = Some(ls);
                }
                processor::Event::LiveTest(lt) => {
                    job.live_test = Some(lt);
                }
                processor::Event::LogCreated(log) => {
                    job.logs.insert(log.kind.as_str().to_string(), log);
                }
            }
        }
        tracing::info!("event stream finished, retrieving outcome");
        let outcome = progress.wait().await;

        let mut job = job.lock().await;
        job.outcome = Some(outcome);
    });

    resp
}

async fn get_job(state: Arc<State>, id: Uuid) -> anyhow::Result<judge_apis::rest::JudgeJob> {
    let job = {
        let jobs = state.judge.read().await;
        match jobs.get(&id) {
            Some(job) => job.clone(),
            None => {
                return Err(anyhow::Error::new(ApiError::new(
                    ErrorKind::NotFound,
                    "JudgeJobNotFound",
                )));
            }
        }
    };
    let job = job.lock().await;
    Ok(job.as_rest())
}

async fn get_job_judge_log(
    state: Arc<State>,
    id: Uuid,
    kind: String,
) -> anyhow::Result<judge_apis::judge_log::JudgeLog> {
    let job = {
        let jobs = state.judge.read().await;
        match jobs.get(&id) {
            Some(job) => job.clone(),
            None => {
                return Err(anyhow::Error::new(ApiError::new(
                    ErrorKind::NotFound,
                    "JudgeJobNotFound",
                )));
            }
        }
    };
    let job = job.lock().await;
    let log = match job.logs.get(&kind) {
        Some(l) => l,
        None => {
            return Err(anyhow::Error::new(ApiError::new(
                ErrorKind::NotFound,
                "JudgeLogNotFound",
            )));
        }
    };
    Ok(log.clone())
}

/// Serves api
#[tracing::instrument(skip(cfg, clients, settings))]
pub async fn serve(
    cfg: RestConfig,
    clients: processor::Clients,
    settings: processor::Settings,
) -> anyhow::Result<()> {
    let state = Arc::new(State {
        judge: RwLock::new(HashMap::new()),
        clients,
        settings,
    });
    let state2 = state.clone();
    let route_create_job = warp::post()
        .and(warp::path("jobs"))
        .and(warp::path::end())
        .and(warp::filters::body::json())
        .and_then(move |req| start_job(state2.clone(), req).map(Result::<_, Infallible>::Ok))
        .map(|resp| warp::reply::json(&resp))
        .boxed();

    let state2 = state.clone();

    let route_get_job = warp::get()
        .and(warp::path("jobs"))
        .and(warp::path::param())
        .and(warp::path::end())
        .and_then(move |id| {
            get_job(state2.clone(), id)
                .map_err(|err| warp::reject::custom(api_util::AnyhowRejection(err)))
        })
        .map(|resp| warp::reply::json(&resp))
        .recover(api_util::recover)
        .boxed();

    let route_get_log = warp::get()
        .and(warp::path("jobs"))
        .and(warp::path::param::<Uuid>())
        .and(warp::path("logs"))
        .and(warp::path::param::<String>())
        .and(warp::path::end())
        .and_then(move |job_id, log_kind| {
            get_job_judge_log(state.clone(), job_id, log_kind)
                .map_err(|err| warp::reject::custom(api_util::AnyhowRejection(err)))
        })
        .map(|resp| warp::reply::json(&resp))
        .recover(api_util::recover)
        .boxed();

    let routes = route_create_job.or(route_get_job).or(route_get_log);

    let server = warp::serve(routes.with(warp::filters::trace::request()));

    let srv = server
        .try_bind_with_graceful_shutdown(([0, 0, 0, 0], cfg.port), futures::future::pending())
        .context("failed to bind")?
        .1;
    srv.await;
    Ok(())
}
