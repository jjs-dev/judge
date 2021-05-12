use crate::exec_test::{ExecOutcome, ResourceUsage};
use anyhow::Context;
use judge_apis::judge_log;
use std::collections::HashMap;
use valuer_api::{status_codes, Status, StatusKind, TestVisibleComponents};

/// Go from valuer judge log to invoker judge log
pub(crate) async fn transform(
    valuer_log: &valuer_api::JudgeLog,
    compile_result: &crate::compile::BuildOutcome,
    test_results: &[(pom::TestId, crate::exec_test::ExecOutcome)],
    problem: &pom::Problem,
    file_ref_resolver: &crate::FileRefResolver,
) -> anyhow::Result<judge_log::JudgeLog> {
    let resource_usage_by_test = {
        let mut map = std::collections::HashMap::new();
        for (k, v) in test_results {
            map.insert(*k, v.resource_usage);
        }
        map
    };
    let mut persistent_judge_log = judge_log::JudgeLog::default();
    let status = if valuer_log.is_full {
        Status {
            kind: StatusKind::Accepted,
            code: status_codes::ACCEPTED.to_string(),
        }
    } else {
        Status {
            kind: StatusKind::Rejected,
            code: status_codes::PARTIAL_SOLUTION.to_string(),
        }
    };
    persistent_judge_log.status = status;
    persistent_judge_log.kind = valuer_log.kind;
    persistent_judge_log.score = valuer_log.score;
    persistent_judge_log.compile_log = compile_result.log.clone();
    // for each test, if valuer allowed, add stdin/stdout/stderr etc to judge_log
    for item in &valuer_log.tests {
        let exec_outcome = test_results
            .iter()
            .find(|(tid, _)| *tid == item.test_id)
            .map(|(_, outcome)| outcome);
        let new_item = export_test(
            item,
            exec_outcome,
            &resource_usage_by_test,
            problem,
            file_ref_resolver,
        )
        .await?;
        persistent_judge_log.tests.push(new_item);
    }
    persistent_judge_log.tests.sort_by_key(|a| a.test_id);

    // note that we do not filter subtasks connected staff,
    // because such filtering is done by Valuer.
    for item in &valuer_log.subtasks {
        persistent_judge_log
            .subtasks
            .push(judge_log::JudgeLogSubtaskRow {
                subtask_id: item.subtask_id,
                score: Some(item.score),
            });
    }
    persistent_judge_log
        .subtasks
        .sort_by_key(|a| a.subtask_id.0);

    Ok(persistent_judge_log)
}

async fn export_test(
    item: &valuer_api::JudgeLogTestRow,
    exec_outcome: Option<&ExecOutcome>,
    resource_usage_by_test: &HashMap<pom::TestId, ResourceUsage>,
    problem: &pom::Problem,
    file_ref_resolver: &crate::FileRefResolver,
) -> anyhow::Result<judge_log::JudgeLogTestRow> {
    let mut new_item = judge_log::JudgeLogTestRow {
        test_id: item.test_id,
        test_answer: None,
        test_stdout: None,
        test_stderr: None,
        test_stdin: None,
        status: None,
        time_usage: None,
        memory_usage: None,
    };
    if item.components.contains(TestVisibleComponents::STATUS) {
        new_item.status = Some(item.status.clone());
    }
    let exec_outcome = match exec_outcome {
        Some(eo) => eo,
        None => return Ok(new_item),
    };

    if item.components.contains(TestVisibleComponents::TEST_DATA) {
        let test_file = &problem.tests[item.test_id].path;
        let test_file = file_ref_resolver.resolve_asset(&test_file);
        let test_data = tokio::fs::read(test_file)
            .await
            .context("failed to read test data")?;
        let test_data = base64::encode(&test_data);
        new_item.test_stdin = Some(test_data);
    }
    if item.components.contains(TestVisibleComponents::OUTPUT) {
        let sol_stdout = base64::encode(&exec_outcome.stdout);
        let sol_stderr = base64::encode(&exec_outcome.stderr);
        new_item.test_stdout = Some(sol_stdout);
        new_item.test_stderr = Some(sol_stderr);
    }
    if item.components.contains(TestVisibleComponents::ANSWER) {
        let answer_ref = &problem.tests[item.test_id].correct;
        if let Some(answer_ref) = answer_ref {
            let answer_file = file_ref_resolver.resolve_asset(answer_ref);
            let answer = tokio::fs::read(answer_file)
                .await
                .context("failed to read correct answer")?;
            let answer = base64::encode(&answer);
            new_item.test_answer = Some(answer);
        }
    }
    if let Some(resource_usage) = resource_usage_by_test.get(&item.test_id) {
        if item
            .components
            .contains(TestVisibleComponents::RESOURCE_USAGE)
        {
            new_item.memory_usage = resource_usage.memory;
            new_item.time_usage = resource_usage.time;
        }
    }
    Ok(new_item)
}
