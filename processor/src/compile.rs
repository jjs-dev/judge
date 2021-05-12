use crate::CommandStatus;
use invoker_api::{
    invoke::{
        Action, ActionResult, Command, EnvVarValue, EnvironmentVariable, Extensions, FileId,
        InvokeRequest, Limits, OutputRequest, OutputRequestTarget, SandboxSettings, SharedDir,
        SharedDirectoryMode, Stdio, Step,
    },
    shim::{SandboxSettingsExtensions, EXTRA_FILES_DIR_NAME},
};
use std::{collections::HashMap, path::PathBuf};
use uuid::Uuid;
use valuer_api::{status_codes, Status, StatusKind};

pub(crate) struct BuildOutcome {
    pub(crate) result: Result<(), Status>,
    pub(crate) log: String,
}

//const FILE_ID_SOURCE: &str = "run-source";
const FILE_ID_EMPTY: &str = "empty";
const SANDBOX_NAME: &str = "compile-sandbox";

pub(crate) async fn compile(
    req: &crate::Request,
    toolchain: &toolchain_loader::Toolchain,
    client: invoker_client::Client,
) -> anyhow::Result<BuildOutcome> {
    let req_builder = crate::request_builder::RequestBuilder::new();

    let (substitutions, extra_files) = {
        let source_file_path = format!("/compile-input/{}", toolchain.spec.filename);
        let mut s = HashMap::new();
        let mut ef = HashMap::new();
        ef.insert(
            toolchain.spec.filename.clone(),
            req_builder.intern(&req.run_source).await?,
        );
        s.insert("Run.SourceFilePath".to_string(), source_file_path.clone());
        s.insert("Run.BinaryFilePath".to_string(), "/jjs/bin".to_string());
        (s, ef)
    };
    let mut invoke_request = InvokeRequest {
        steps: vec![],
        inputs: vec![],
        outputs: vec![],
        id: Uuid::nil(),
        ext: Extensions::make(invoker_api::shim::RequestExtensions {
            extra_files,
            substitutions,
        })?,
    };

    invoke_request.steps.push(Step {
        stage: 0,
        action: Action::OpenNullFile {
            id: FileId(FILE_ID_EMPTY.to_string()),
        },
        ext: Extensions::default(),
    });

    let limits = Limits {
        memory: toolchain.spec.limits.memory(),
        time: toolchain.spec.limits.time(),
        process_count: toolchain.spec.limits.process_count,
        work_dir_size: toolchain.spec.limits.work_dir_size,
        ext: Extensions::default(),
    };
    invoke_request.steps.push(Step {
        stage: 0,
        action: Action::CreateSandbox(SandboxSettings {
            limits: limits.clone(),
            name: SANDBOX_NAME.to_string(),
            base_image: PathBuf::new(),
            expose: vec![SharedDir {
                host_path: EXTRA_FILES_DIR_NAME.into(),
                sandbox_path: "/compile-input".into(),
                mode: SharedDirectoryMode::ReadOnly,
                create: false,
                ext: Extensions::default(),
            }],
            work_dir: PathBuf::new(),
            ext: Extensions::make(SandboxSettingsExtensions {
                image: toolchain.image.clone(),
            })?,
        }),
        ext: Extensions::default(),
    });
    let mut command_steps = Vec::new();

    for (i, command) in toolchain.spec.build_commands.iter().enumerate() {
        let stdout_file_id = format!("step-{}-stdout", i);
        let stderr_file_id = format!("step-{}-stderr", i);
        invoke_request.steps.push(Step {
            stage: i as u32,
            action: Action::CreateFile {
                id: FileId(stdout_file_id.clone()),
                readable: true,
                writeable: true,
            },
            ext: Extensions::default(),
        });
        invoke_request.steps.push(Step {
            stage: i as u32,
            action: Action::CreateFile {
                id: FileId(stderr_file_id.clone()),
                readable: true,
                writeable: true,
            },
            ext: Extensions::default(),
        });
        let inv_cmd = Command {
            sandbox_name: SANDBOX_NAME.to_string(),
            argv: command.argv.clone(),
            env: command
                .env
                .clone()
                .into_iter()
                .map(|(k, v)| EnvironmentVariable {
                    name: k,
                    value: EnvVarValue::Plain(v),
                    ext: Extensions::default(),
                })
                .collect(),
            cwd: "/jjs".to_string(),
            stdio: Stdio {
                stdin: FileId(FILE_ID_EMPTY.to_string()),
                stdout: FileId(stdout_file_id.clone()),
                stderr: FileId(stderr_file_id.clone()),
                ext: Extensions::default(),
            },
            ext: Extensions::default(),
        };

        command_steps.push(invoke_request.steps.len());
        invoke_request.steps.push(Step {
            stage: i as u32,
            action: Action::ExecuteCommand(inv_cmd),
            ext: Extensions::default(),
        });

        invoke_request.outputs.push(OutputRequest {
            name: stdout_file_id.clone(),
            target: OutputRequestTarget::File(FileId(stdout_file_id.clone())),
            ext: Extensions::default(),
        });
        invoke_request.outputs.push(OutputRequest {
            name: stderr_file_id.clone(),
            target: OutputRequestTarget::File(FileId(stderr_file_id.clone())),
            ext: Extensions::default(),
        });
    }
    let response = client.instance()?.call(invoke_request).await?;
    let mut compile_log = String::new();
    for (step_no, pos) in command_steps.into_iter().enumerate() {
        let data = match &response.actions[pos] {
            ActionResult::ExecuteCommand(d) => d,
            _ => anyhow::bail!("unexpected ActionResult"),
        };

        let stdout = req_builder
            .read_output(&response, &format!("step-{}-stdout", step_no))
            .await?;
        let stderr = req_builder
            .read_output(&response, &format!("step-{}-stderr", step_no))
            .await?;
        compile_log += &format!("------ step {} ------\n", step_no);
        compile_log += "--- stdout ---\n";
        compile_log += &String::from_utf8_lossy(&stdout);
        compile_log += "--- stderr ---\n";
        compile_log += &String::from_utf8_lossy(&stderr);

        let status_code = match crate::describe_command_result(&limits, data) {
            // TODO: use more specific status
            CommandStatus::MemLimit => status_codes::COMPILER_FAILED,
            CommandStatus::Startup => status_codes::COMPILER_FAILED,
            CommandStatus::Runtime => status_codes::COMPILER_FAILED,
            CommandStatus::TimeLimit => status_codes::COMPILATION_TIMED_OUT,
            CommandStatus::Ok => continue,
        };
        return Ok(BuildOutcome {
            result: Err(Status {
                kind: StatusKind::CompilationError,
                code: status_code.to_string(),
            }),
            log: compile_log,
        });
    }
    Ok(BuildOutcome {
        result: Ok(()),
        log: compile_log,
    })
}
