//! Pure implementation that returns valid, but fake data

use crate::{JobProgress, ProtocolSender, Request};
use judge_apis::judge_log::{JudgeLog, JudgeLogSubtaskRow, JudgeLogTestRow};
use pom::TestId;
use rand::{
    distributions::{Alphanumeric, Uniform},
    prelude::SliceRandom,
    Rng, SeedableRng,
};
use rand_chacha::ChaChaRng;
use std::hash::{Hash, Hasher};
use tokio::sync::{mpsc, oneshot};
use valuer_api::{status_codes, JudgeLogKind, Status, StatusKind, SubtaskId};

#[derive(Clone)]
pub struct FakeSettings {}

pub fn judge(req: Request, settings: FakeSettings) -> JobProgress {
    let (done_tx, done_rx) = oneshot::channel();
    let (events_tx, events_rx) = mpsc::channel(1);
    tokio::task::spawn(async move {
        let mut protocol_sender = ProtocolSender {
            sent: Vec::new(),
            tx: events_tx,
            debug_dump_dir: None,
        };

        do_judge(req, &mut protocol_sender, settings).await;

        done_tx.send(Ok(())).ok();
    });
    JobProgress { events_rx, done_rx }
}

fn stable_hash<T: Hash + ?Sized>(val: &T) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    val.hash(&mut h);
    h.finish()
}

fn generate_string((len_lo, len_hi): (usize, usize), rng: &mut ChaChaRng) -> String {
    let dist = Uniform::new(len_lo, len_hi);
    let len = rng.sample(dist);

    (0..len).map(|_| rng.sample(Alphanumeric) as char).collect()
}

fn generate_judge_log(kind: JudgeLogKind, rng: &mut ChaChaRng) -> JudgeLog {
    let test_count = rng.sample(Uniform::new(3_u32, 20));
    let make_status = |rng: &mut ChaChaRng| Status {
        kind: [StatusKind::Accepted, StatusKind::Rejected]
            .choose(&mut *rng)
            .copied()
            .unwrap(),
        code: [
            status_codes::WRONG_ANSWER,
            status_codes::TEST_PASSED,
            status_codes::TIME_LIMIT_EXCEEDED,
            status_codes::RUNTIME_ERROR,
        ]
        .choose(&mut *rng)
        .copied()
        .unwrap()
        .to_owned(),
    };
    let tests = (0..test_count)
        .map(|id| JudgeLogTestRow {
            test_id: TestId::make(id + 1),
            status: Some(make_status(&mut *rng)),
            test_stdin: Some(generate_string((3, 100), rng)),
            test_stdout: Some(generate_string((3, 100), rng)),
            test_stderr: Some(generate_string((3, 100), rng)),
            test_answer: Some(generate_string((3, 100), rng)),
            time_usage: Some(rng.sample(Uniform::new(1_000_000, 1_000_000_000))),
            memory_usage: Some(rng.sample(Uniform::new(1_000_000, 1_000_000_000))),
        })
        .collect();
    let subtask_count = rng.sample(Uniform::new(1_u32, 10));
    let subtasks = (0..subtask_count)
        .map(|id| JudgeLogSubtaskRow {
            subtask_id: SubtaskId::make(id + 1),
            score: Some(rng.sample(Uniform::new(0, 100))),
        })
        .collect();
    JudgeLog {
        kind,
        tests,
        subtasks,
        score: rng.sample(Uniform::new(0, 100)),
        status: make_status(rng),
        compile_log: (generate_string((10, 200), rng)),
        is_full: false,
    }
}

async fn do_judge(req: Request, protocol_sender: &mut ProtocolSender, _settings: FakeSettings) {
    for kind in JudgeLogKind::list() {
        let seed = stable_hash(&(&req.toolchain_name, &req.run_source, kind.as_str()));
        tracing::info!(kind = kind.as_str(), seed = seed, "generating judge log");
        let mut rng = ChaChaRng::seed_from_u64(seed);
        let log = generate_judge_log(kind, &mut rng);
        protocol_sender.send_log(log).await;
    }
}
