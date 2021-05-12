mod checker_proto;

use std::path::PathBuf;

use anyhow::Context;
use invoker_api::invoke::{
    Action, ActionResult, Command, EnvVarValue, EnvironmentVariable, Extensions, FileId, Input,
    InvokeRequest, Limits, SandboxSettings, Stdio, Step,
};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;
use valuer_api::{status_codes, Status, StatusKind};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ResourceUsage {
    pub(crate) memory: Option<u64>,
    pub(crate) time: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct ExecOutcome {
    pub(crate) status: Status,
    pub(crate) resource_usage: ResourceUsage,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

fn map_checker_outcome_to_status(out: checker_proto::Output) -> Status {
    match out.outcome {
        checker_proto::Outcome::Ok => Status {
            kind: StatusKind::Accepted,
            code: status_codes::TEST_PASSED.to_string(),
        },
        checker_proto::Outcome::BadChecker => Status {
            kind: StatusKind::InternalError,
            code: status_codes::JUDGE_FAULT.to_string(),
        },
        checker_proto::Outcome::PresentationError => Status {
            kind: StatusKind::Rejected,
            code: status_codes::PRESENTATION_ERROR.to_string(),
        },
        checker_proto::Outcome::WrongAnswer => Status {
            kind: StatusKind::Rejected,
            code: status_codes::WRONG_ANSWER.to_string(),
        },
    }
}

/// Runs Artifact on one test and produces output
pub(crate) async fn exec(
    toolchain: &toolchain_loader::Toolchain,
    problem: &pom::Problem,
    client: invoker_client::Client,
    file_ref_resolver: &crate::FileRefResolver,
    test_id: pom::TestId,
    debug: &crate::DebugDumps,
) -> anyhow::Result<ExecOutcome> {
    let test = problem
        .tests
        .get(test_id.to_idx())
        .context("unknown test")?;
    let mut invoke_request = InvokeRequest {
        steps: vec![],
        inputs: vec![],
        outputs: vec![],
        id: Uuid::nil(),
        ext: Extensions::default(),
    };
    let req_builder = crate::request_builder::RequestBuilder::new();

    let test_file = file_ref_resolver.resolve_asset(&test.path);
    //let test_data = tokio::fs::read(input_file).await.context("failed to read test")?;
    const PREPARE_STAGE: u32 = 0;
    const EXEC_SOLUTION_STAGE: u32 = 1;
    const TEST_DATA_INPUT_FILE: &str = "test-data";
    const EXEC_SOLUTION_OUTPUT_FILE: &str = "solution-output";
    const EXEC_SOLUTION_ERROR_FILE: &str = "solution-error";
    const CORRECT_ANSWER_FILE: &str = "correct";
    const EMPTY_FILE: &str = "empty";

    const SOLUTION_SANDBOX_NAME: &str = "exec-sandbox";
    const CHECKER_SANDBOX_NAME: &str = "checker-sandbox";

    const EXEC_CHECKER_STAGE: u32 = 2;
    // create an input with the test data

    let test_data_input = Input {
        file_id: FileId(TEST_DATA_INPUT_FILE.to_string()),
        source: req_builder.intern_file(&test_file).await?,
        ext: Extensions::default(),
    };
    invoke_request.inputs.push(test_data_input);

    // prepare empty input

    invoke_request.steps.push(Step {
        stage: PREPARE_STAGE,
        action: Action::OpenNullFile {
            id: FileId(EMPTY_FILE.to_string()),
        },
        ext: Extensions::default(),
    });

    // prepare files for stdout & stderr

    invoke_request.steps.push(Step {
        stage: EXEC_SOLUTION_STAGE,
        action: Action::CreateFile {
            id: FileId(EXEC_SOLUTION_OUTPUT_FILE.to_string()),
            readable: false,
            writeable: true,
        },
        ext: Extensions::default(),
    });
    invoke_request.steps.push(Step {
        stage: EXEC_SOLUTION_STAGE,
        action: Action::CreateFile {
            id: FileId(EXEC_SOLUTION_ERROR_FILE.to_string()),
            readable: false,
            writeable: true,
        },
        ext: Extensions::default(),
    });

    let exec_solution_step_id = invoke_request.steps.len();

    // create sandbox
    invoke_request.steps.push(Step {
        stage: EXEC_SOLUTION_STAGE,
        action: Action::CreateSandbox(SandboxSettings {
            limits: Limits {
                memory: test.limits.memory(),
                time: test.limits.time(),
                process_count: Some(test.limits.process_count()),
                work_dir_size: Some(test.limits.work_dir_size()),
                ext: Extensions::default(),
            },
            name: SOLUTION_SANDBOX_NAME.to_string(),
            base_image: PathBuf::new(),
            expose: Vec::new(),
            work_dir: PathBuf::new(),
            ext: Extensions::default(),
        }),
        ext: Extensions::default(),
    });
    // produce a step for executing solution
    {
        let exec_solution_step = Step {
            stage: EXEC_SOLUTION_STAGE,
            action: Action::ExecuteCommand(Command {
                sandbox_name: SOLUTION_SANDBOX_NAME.to_string(),
                argv: toolchain.spec.run_command.argv.clone(),
                env: toolchain
                    .spec
                    .run_command
                    .env
                    .iter()
                    .map(|(k, v)| EnvironmentVariable {
                        name: k.clone(),
                        value: EnvVarValue::Plain(v.clone()),
                        ext: Extensions::default(),
                    })
                    .collect(),
                cwd: toolchain.spec.run_command.cwd.clone(),
                stdio: Stdio {
                    stdin: FileId(TEST_DATA_INPUT_FILE.to_string()),
                    stdout: FileId(EXEC_SOLUTION_OUTPUT_FILE.to_string()),
                    stderr: FileId(EXEC_SOLUTION_ERROR_FILE.to_string()),
                    ext: Extensions::default(),
                },
                ext: Extensions::default(),
            }),
            ext: Extensions::default(),
        };
        invoke_request.steps.push(exec_solution_step);
    }
    // provide a correct answer if requested
    {
        let source = if let Some(corr_path) = &test.correct {
            let full_path = file_ref_resolver.resolve_asset(corr_path);
            let data = tokio::fs::read(full_path)
                .await
                .context("failed to read correct answer")?;
            req_builder.intern(&data).await?
        } else {
            req_builder.intern(&[]).await?
        };
        invoke_request.inputs.push(Input {
            file_id: FileId(CORRECT_ANSWER_FILE.to_string()),
            source,
            ext: Extensions::default(),
        })
    }
    // generate checker feedback files
    const CHECKER_DECISION: &str = "checker-decision";
    const CHECKER_LOG: &str = "checker-logs";

    invoke_request.steps.push(Step {
        stage: EXEC_CHECKER_STAGE,
        action: Action::CreateFile {
            id: FileId(CHECKER_DECISION.to_string()),
            readable: false,
            writeable: true,
        },
        ext: Extensions::default(),
    });
    invoke_request.steps.push(Step {
        stage: EXEC_CHECKER_STAGE,
        action: Action::CreateFile {
            id: FileId(CHECKER_LOG.to_string()),
            readable: false,
            writeable: true,
        },
        ext: Extensions::default(),
    });

    // produce a step for executing checker
    let exec_checker_test_id = invoke_request.steps.len();
    invoke_request.steps.push(Step {
        stage: EXEC_CHECKER_STAGE,
        action: Action::ExecuteCommand(Command {
            argv: problem.checker_cmd.clone(),
            env: vec![
                EnvironmentVariable {
                    name: "JJS_CORR".to_string(),
                    value: EnvVarValue::File(FileId(CORRECT_ANSWER_FILE.to_string())),
                    ext: Extensions::default(),
                },
                EnvironmentVariable {
                    name: "JJS_SOL".to_string(),
                    value: EnvVarValue::File(FileId(EXEC_SOLUTION_OUTPUT_FILE.to_string())),
                    ext: Extensions::default(),
                },
                EnvironmentVariable {
                    name: "JJS_TEST".to_string(),
                    value: EnvVarValue::File(FileId(TEST_DATA_INPUT_FILE.to_string())),
                    ext: Extensions::default(),
                },
                EnvironmentVariable {
                    name: "JJS_CHECKER_OUT".to_string(),
                    value: EnvVarValue::File(FileId(CHECKER_DECISION.to_string())),
                    ext: Extensions::default(),
                },
                EnvironmentVariable {
                    name: "JJS_CHECKER_COMMENT".to_string(),
                    value: EnvVarValue::File(FileId(CHECKER_LOG.to_string())),
                    ext: Extensions::default(),
                },
            ]
            .into_iter()
            .collect(),
            cwd: "/".to_string(),
            stdio: Stdio {
                stdin: FileId(EMPTY_FILE.to_string()),
                stdout: FileId(EMPTY_FILE.to_string()),
                stderr: FileId(EMPTY_FILE.to_string()),
                ext: Extensions::default(),
            },
            ext: Extensions::default(),
            sandbox_name: CHECKER_SANDBOX_NAME.to_string(),
        }),
        ext: Extensions::default(),
    });

    let response = client.instance()?.call(invoke_request).await?;

    if let Some(dir) = &debug.checker_logs {
        let checker_out_file = tokio::fs::File::create(dir.join(test_id.to_string())).await?;
        let mut checker_out_file = tokio::io::BufWriter::new(checker_out_file);
        let checker_logs = req_builder.read_output(&response, CHECKER_LOG).await?;
        checker_out_file.write_all(&checker_logs).await?;
    }

    let make_return_value_for_judge_fault = || {
        Ok(ExecOutcome {
            status: Status {
                kind: StatusKind::InternalError,
                code: status_codes::JUDGE_FAULT.to_string(),
            },
            resource_usage: Default::default(),
            stdout: String::new(),
            stderr: String::new(),
        })
    };

    let solution_command_result = {
        let res = response
            .actions
            .get(exec_solution_step_id)
            .context("bug: invalid index")?;
        match res {
            ActionResult::ExecuteCommand(cmd) => cmd,
            _ => anyhow::bail!("bug: unexpected action result"),
        }
    };

    let solution_stdout = req_builder
        .read_output(&response, EXEC_SOLUTION_OUTPUT_FILE)
        .await?;
    let solution_stderr = req_builder
        .read_output(&response, EXEC_SOLUTION_ERROR_FILE)
        .await?;

    let checker_command_result = {
        let res = response
            .actions
            .get(exec_checker_test_id)
            .context("bug: invalid index")?;
        match res {
            ActionResult::ExecuteCommand(cmd) => cmd,
            _ => anyhow::bail!("bug: unexpected action result"),
        }
    };

    let checker_success = checker_command_result.exit_code == 0;
    if !checker_success {
        tracing::error!(
            "checker returned non-zero: {}",
            checker_command_result.exit_code
        );
        return make_return_value_for_judge_fault();
    }

    let checker_out = req_builder.read_output(&response, CHECKER_DECISION).await?;

    let checker_out = match String::from_utf8(checker_out) {
        Ok(c) => c,
        Err(_) => {
            tracing::error!("checker produced non-utf8 output");
            return make_return_value_for_judge_fault();
        }
    };
    let parsed_out = match checker_proto::parse(&checker_out) {
        Ok(o) => o,
        Err(err) => {
            tracing::error!("checker output couldn't be parsed: {}", err);
            return make_return_value_for_judge_fault();
        }
    };

    let status = map_checker_outcome_to_status(parsed_out);

    let resource_usage = ResourceUsage {
        memory: solution_command_result.memory,
        time: solution_command_result.cpu_time,
    };

    Ok(ExecOutcome {
        status,
        resource_usage,
        stdout: String::from_utf8_lossy(&solution_stdout).into_owned(),
        stderr: String::from_utf8_lossy(&solution_stderr).into_owned(),
    })
}
