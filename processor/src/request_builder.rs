use anyhow::Context;
use invoker_api::invoke::{InputSource, InvokeResponse, OutputData};
use std::path::Path;

/// Utility for exchanging data with invoker.
pub(crate) struct RequestBuilder {}

impl RequestBuilder {
    pub fn new() -> Self {
        RequestBuilder {}
    }

    pub async fn intern(&self, data: &[u8]) -> anyhow::Result<InputSource> {
        // TODO: use LocalFile when possible
        Ok(InputSource::InlineBase64 {
            data: base64::encode(data),
        })
    }

    pub async fn intern_file(&self, path: &Path) -> anyhow::Result<InputSource> {
        // TODO: be smarter here
        let data = tokio::fs::read(path).await?;
        self.intern(&data).await
    }

    pub async fn read_output_data(&self, out: &OutputData) -> anyhow::Result<Vec<u8>> {
        match out {
            OutputData::InlineBase64(b) => base64::decode(b).context("invalid base64"),
        }
    }

    pub async fn read_output(
        &self,
        res: &InvokeResponse,
        output_name: &str,
    ) -> anyhow::Result<Vec<u8>> {
        let output = res
            .outputs
            .iter()
            .find(|o| o.name == output_name)
            .with_context(|| format!("output {} not found", output_name))?;
        let data = self.read_output_data(&output.data).await?;
        Ok(data)
    }
}
