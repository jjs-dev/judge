mod checker_proto;

use anyhow::Context;
use invoker_api::{
    invoke::{
        Action, ActionResult, Command, EnvVarValue, EnvironmentVariable, Extensions, FileId, Input,
        InvokeRequest, Limits, OutputRequest, OutputRequestTarget, PathPrefix, PrefixedPath,
        SandboxSettings, SharedDir, SharedDirectoryMode, Stdio, Step,
    },
    shim::{
        ExtraFile, RequestExtensions, SandboxSettingsExtensions, SharedDirExtensionSource,
        EXTRA_FILES_DIR_NAME,
    },
};
use std::{collections::HashMap, path::PathBuf};
use uuid::Uuid;
use valuer_api::{status_codes, Status, StatusKind};

use crate::compile::BuiltRun;

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

const CHECKER_DECISION: &str = "checker-decision";
const CHECKER_LOG: &str = "checker-logs";

struct StepIds {
    exec_solution: usize,
    exec_checker: usize,
}

async fn create_request(
    toolchain: &toolchain_loader::Toolchain,
    problem: &pom::Problem,
    file_ref_resolver: &crate::FileRefResolver,
    test: &pom::Test,
    req_builder: &crate::request_builder::RequestBuilder,
    built: &BuiltRun,
) -> anyhow::Result<(InvokeRequest, StepIds)> {
    let (substitutions, extra_files) = {
        let mut s = HashMap::new();
        let mut ef = HashMap::new();
        let test_path = file_ref_resolver.resolve_asset(&test.path);
        ef.insert(
            "exec/test".to_string(),
            ExtraFile {
                contents: req_builder.intern_file(&test_path).await?,
                executable: false,
            },
        );
        ef.insert(
            "compile-out/bin".to_string(),
            ExtraFile {
                contents: req_builder.intern(&built.binary).await?,
                executable: true,
            },
        );
        let checker = file_ref_resolver.resolve_asset(&problem.checker_exe);
        ef.insert(
            "check/checker".to_string(),
            ExtraFile {
                contents: req_builder.intern_file(&checker).await?,
                executable: true,
            },
        );
        s.insert(
            "Run.BinaryFilePath".to_string(),
            "/compile-out/bin".to_string(),
        );
        (s, ef)
    };
    let mut invoke_request = InvokeRequest {
        steps: vec![],
        inputs: vec![],
        outputs: vec![],
        id: Uuid::nil(),
        ext: Extensions::make(RequestExtensions {
            extra_files,
            substitutions,
        })?,
    };

    let test_file = file_ref_resolver.resolve_asset(&test.path);
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
            readable: true,
            writeable: true,
        },
        ext: Extensions::default(),
    });
    invoke_request.steps.push(Step {
        stage: EXEC_SOLUTION_STAGE,
        action: Action::CreateFile {
            id: FileId(EXEC_SOLUTION_ERROR_FILE.to_string()),
            readable: true,
            writeable: true,
        },
        ext: Extensions::default(),
    });

    // create solution sandbox
    invoke_request.steps.push(Step {
        stage: EXEC_SOLUTION_STAGE,
        action: Action::CreateSandbox(SandboxSettings {
            limits: Limits {
                memory: test.limits.memory(),
                time: test.limits.time(),
                process_count: Some(test.limits.process_count()),
                ext: Extensions::default(),
            },
            name: SOLUTION_SANDBOX_NAME.to_string(),
            base_image: PathBuf::new(),
            expose: vec![SharedDir {
                host_path: PrefixedPath {
                    prefix: PathPrefix::Extension(Extensions::make(SharedDirExtensionSource {
                        name: EXTRA_FILES_DIR_NAME.to_string(),
                    })?),
                    path: "compile-out".into(),
                },
                sandbox_path: "/compile-out".into(),
                mode: SharedDirectoryMode::ReadOnly,
                create: false,
                ext: Extensions::default(),
            }],
            ext: Extensions::make(SandboxSettingsExtensions {
                image: toolchain.image.clone(),
            })?,
        }),
        ext: Extensions::default(),
    });

    // produce a step for executing solution
    let exec_solution_step_id = invoke_request.steps.len();

    invoke_request.steps.push(Step {
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
    });

    // provide a correct answer if requested
    let has_correct_answer;
    {
        if let Some(corr_path) = &test.correct {
            let full_path = file_ref_resolver.resolve_asset(corr_path);
            let source = req_builder.intern_file(&full_path).await?;

            has_correct_answer = true;

            invoke_request.inputs.push(Input {
                file_id: FileId(CORRECT_ANSWER_FILE.to_string()),
                source,
                ext: Extensions::default(),
            })
        } else {
            has_correct_answer = false;
        }
    }
    // generate checker feedback files

    invoke_request.steps.push(Step {
        stage: EXEC_CHECKER_STAGE,
        action: Action::CreateFile {
            id: FileId(CHECKER_DECISION.to_string()),
            readable: true,
            writeable: true,
        },
        ext: Extensions::default(),
    });
    invoke_request.steps.push(Step {
        stage: EXEC_CHECKER_STAGE,
        action: Action::CreateFile {
            id: FileId(CHECKER_LOG.to_string()),
            readable: true,
            writeable: true,
        },
        ext: Extensions::default(),
    });

    // create a checker sandbox
    invoke_request.steps.push(Step {
        stage: EXEC_CHECKER_STAGE,
        action: Action::CreateSandbox(SandboxSettings {
            limits: Limits {
                memory: test.limits.memory(),
                time: test.limits.time(),
                process_count: Some(test.limits.process_count()),
                ext: Extensions::default(),
            },
            name: CHECKER_SANDBOX_NAME.to_string(),
            base_image: PathBuf::new(),
            expose: vec![SharedDir {
                host_path: PrefixedPath {
                    prefix: PathPrefix::Extension(Extensions::make(SharedDirExtensionSource {
                        name: EXTRA_FILES_DIR_NAME.to_string(),
                    })?),
                    path: "check".into(),
                },
                sandbox_path: "/check".into(),
                mode: SharedDirectoryMode::ReadOnly,
                create: false,
                ext: Extensions::default(),
            }],
            ext: Extensions::make(SandboxSettingsExtensions {
                // TODO: allow overriding
                image: "gcr.io/distroless/cc:latest".to_string(),
            })?,
        }),
        ext: Extensions::default(),
    });

    // produce a step for executing checker
    let exec_checker_test_id = invoke_request.steps.len();

    let mut checker_cmd = vec!["/check/checker".to_string()];
    checker_cmd.extend_from_slice(&problem.checker_cmd);
    let mut checker_env = vec![
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
    ];

    if has_correct_answer {
        checker_env.push(EnvironmentVariable {
            name: "JJS_CORR".to_string(),
            value: EnvVarValue::File(FileId(CORRECT_ANSWER_FILE.to_string())),
            ext: Extensions::default(),
        });
    }

    invoke_request.steps.push(Step {
        stage: EXEC_CHECKER_STAGE,
        action: Action::ExecuteCommand(Command {
            argv: checker_cmd,
            env: checker_env,
            cwd: "/".to_string(),
            stdio: Stdio {
                stdin: FileId(EMPTY_FILE.to_string()),
                stdout: FileId(CHECKER_LOG.to_string()),
                stderr: FileId(CHECKER_LOG.to_string()),
                ext: Extensions::default(),
            },
            ext: Extensions::default(),
            sandbox_name: CHECKER_SANDBOX_NAME.to_string(),
        }),
        ext: Extensions::default(),
    });

    // add output requests
    invoke_request.outputs.push(OutputRequest {
        name: CHECKER_LOG.to_string(),
        target: OutputRequestTarget::File(FileId(CHECKER_LOG.to_string())),
        ext: Extensions::default(),
    });
    invoke_request.outputs.push(OutputRequest {
        name: CHECKER_DECISION.to_string(),
        target: OutputRequestTarget::File(FileId(CHECKER_DECISION.to_string())),
        ext: Extensions::default(),
    });
    invoke_request.outputs.push(OutputRequest {
        name: EXEC_SOLUTION_OUTPUT_FILE.to_string(),
        target: OutputRequestTarget::File(FileId(EXEC_SOLUTION_OUTPUT_FILE.to_string())),
        ext: Extensions::default(),
    });
    invoke_request.outputs.push(OutputRequest {
        name: EXEC_SOLUTION_ERROR_FILE.to_string(),
        target: OutputRequestTarget::File(FileId(EXEC_SOLUTION_ERROR_FILE.to_string())),
        ext: Extensions::default(),
    });

    Ok((
        invoke_request,
        StepIds {
            exec_checker: exec_checker_test_id,
            exec_solution: exec_solution_step_id,
        },
    ))
}

/// Runs Artifact on one test and produces output
pub(crate) async fn exec(
    toolchain: &toolchain_loader::Toolchain,
    problem: &pom::Problem,
    client: invoker_client::Client,
    file_ref_resolver: &crate::FileRefResolver,
    test_id: pom::TestId,
    settings: &crate::Settings,
    built: &BuiltRun,
) -> anyhow::Result<ExecOutcome> {
    let req_builder = crate::request_builder::RequestBuilder::new();

    let test = problem
        .tests
        .get(test_id.to_idx())
        .context("unknown test")?;

    let (invoke_request, step_ids) = create_request(
        toolchain,
        problem,
        file_ref_resolver,
        test,
        &req_builder,
        built,
    )
    .await
    .context("failed to prepare invoke request")?;

    let response = client.instance()?.call(invoke_request).await?;

    tracing::debug!("parsing invoker response");

    if let Some(dir) = &settings.checker_logs {
        tracing::debug!("saving checker log");
        tokio::fs::create_dir_all(&dir)
            .await
            .context("failed to create checker logs directory")?;
        let checker_out_file = dir.join(test_id.get().to_string());
        let checker_logs = req_builder.read_output(&response, CHECKER_LOG).await?;
        tokio::fs::write(checker_out_file, checker_logs).await?;
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
            .get(step_ids.exec_solution)
            .context("bug: invalid index")?;
        match res {
            ActionResult::ExecuteCommand(cmd) => cmd,
            _ => anyhow::bail!("bug: unexpected action result for exec solution step"),
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
            .get(step_ids.exec_checker)
            .context("bug: invalid index")?;
        match res {
            ActionResult::ExecuteCommand(cmd) => cmd,
            _ => anyhow::bail!("bug: unexpected action result for exec checker step"),
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
