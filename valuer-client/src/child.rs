use crate::ChildClientConfig;
use anyhow::Context;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};

pub(crate) struct ChildClient {
    stdin: BufWriter<tokio::process::ChildStdin>,
    stdout: BufReader<tokio::process::ChildStdout>,
    // ties lifetime of valuer instance to `Valuer` lifetime
    _child: tokio::process::Child,
}

impl ChildClient {
    pub(crate) async fn new(cfg: &ChildClientConfig) -> anyhow::Result<Self> {
        let mut cmd = tokio::process::Command::new(&cfg.exe);
        cmd.args(&cfg.args);
        cmd.kill_on_drop(true);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::inherit());
        cmd.env("JJS_VALUER", "1");
        // TODO: this is hack
        cmd.env("RUST_LOG", "info,svaluer=debug");
        let work_dir_exists = tokio::fs::metadata(&cfg.current_dir).await.is_ok();
        if work_dir_exists {
            cmd.current_dir(&cfg.current_dir);
        } else {
            tracing::warn!(
                "Not setting current dir for valuer because path specified ({}) does not exists",
                cfg.current_dir.display()
            );
        }
        let mut child = cmd.spawn().with_context(|| {
            format!(
                "failed to spawn valuer {} (requested current dir {})",
                cfg.exe.display(),
                cfg.current_dir.display()
            )
        })?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let val = ChildClient {
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            _child: child,
        };

        Ok(val)
    }

    async fn write_val(&mut self, msg: impl serde::Serialize) -> anyhow::Result<()> {
        let mut msg = serde_json::to_string(&msg).context("failed to serialize")?;
        if msg.contains('\n') {
            anyhow::bail!("bug: serialized message is not oneline");
        }
        msg.push('\n');
        self.stdin
            .write_all(msg.as_bytes())
            .await
            .context("failed to write message")?;
        self.stdin
            .flush()
            .await
            .context("failed to flush valuer stdin")?;
        Ok(())
    }

    pub(crate) async fn write_problem_data(
        &mut self,
        info: valuer_api::ProblemInfo,
    ) -> anyhow::Result<()> {
        self.write_val(info).await
    }

    pub(crate) async fn poll(&mut self) -> anyhow::Result<valuer_api::ValuerResponse> {
        let mut line = String::new();
        let read_line_fut = self.stdout.read_line(&mut line);
        match tokio::time::timeout(std::time::Duration::from_secs(15), read_line_fut).await {
            Ok(read) => {
                read.context("early eof")?;
            }
            Err(_elapsed) => {
                anyhow::bail!("valuer response timed out");
            }
        }
        let response = serde_json::from_str(&line).context("failed to parse valuer message")?;

        Ok(response)
    }

    pub(crate) async fn notify_test_done(
        &mut self,
        notification: valuer_api::TestDoneNotification,
    ) -> anyhow::Result<()> {
        self.write_val(notification).await
    }
}
