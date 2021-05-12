use std::path::PathBuf;

use child::ChildClient;

mod child;

/// Data, required to create a valuer client.
/// This is a bit lowered version of `pom::Valuer`.
pub enum ClientConfig {
    Child(ChildClientConfig),
}

pub struct ChildClientConfig {
    pub exe: PathBuf,
    pub args: Vec<String>,
    pub log_file: PathBuf,
    pub current_dir: PathBuf,
}

enum Inner {
    Child(ChildClient),
}

/// ValuerClient can be used to communicate with valuer.
pub struct ValuerClient(Inner);

impl ValuerClient {
    pub async fn new(config: &ClientConfig) -> anyhow::Result<Self> {
        let inner = match config {
            ClientConfig::Child(cfg) => Inner::Child(ChildClient::new(cfg).await?),
        };
        Ok(ValuerClient(inner))
    }

    pub async fn write_problem_data(
        &mut self,
        info: valuer_api::ProblemInfo,
    ) -> anyhow::Result<()> {
        match &mut self.0 {
            Inner::Child(inner) => inner.write_problem_data(info).await,
        }
    }

    pub async fn poll(&mut self) -> anyhow::Result<valuer_api::ValuerResponse> {
        match &mut self.0 {
            Inner::Child(inner) => inner.poll().await,
        }
    }

    pub async fn notify_test_done(
        &mut self,
        notification: valuer_api::TestDoneNotification,
    ) -> anyhow::Result<()> {
        match &mut self.0 {
            Inner::Child(inner) => inner.notify_test_done(notification).await,
        }
    }
}
