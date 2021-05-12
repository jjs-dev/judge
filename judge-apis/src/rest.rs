use crate::live::LiveJudgeStatus;
use serde::{de::Error, Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Base64 encoding for binary data
pub struct ByteString(pub Vec<u8>);

impl Serialize for ByteString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let repr = base64::encode(&self.0);
        repr.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ByteString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let repr = String::deserialize(deserializer)?;
        base64::decode(&repr).map(ByteString).map_err(|err| {
            D::Error::custom(format_args!(
                "expected valid base64-encoded string: {:#}",
                err
            ))
        })
    }
}

/// Judge request
#[derive(Serialize, Deserialize)]
pub struct JudgeRequest {
    /// Toolchain name (will be passed to toolchain loader)
    pub toolchain_name: String,
    /// Problem name (will be passed to problem loader)
    pub problem_id: String,
    /// Run source, as a base64-encoded string
    pub run_source: ByteString,
    /// Additional metadata. Judge will simply preserve it.
    #[serde(default)]
    pub annotations: HashMap<String, String>,
}

/// Information about previously created judge job
#[derive(Serialize, Deserialize)]
pub struct JudgeJob {
    /// Identifier of the job
    pub id: Uuid,
    /// Logs that were created
    pub logs: Vec<String>,
    /// Annotations as specified in request
    pub annotations: HashMap<String, String>,
    /// Whether the job has completed
    pub completed: bool,
    /// Live status
    pub live: LiveJudgeStatus,
    /// Error message, if the job has failed
    pub error: Option<String>,
}
