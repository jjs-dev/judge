use serde::{Deserialize, Serialize};

/// Describes current judging status of particular job.
/// This information can be imprecise or stale, so it should
/// not be relied upon.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LiveJudgeStatus {
    /// Current test. If run is being tested on multiple tests,
    /// it is unspecified which is returned
    pub test: Option<u32>,
    /// Current score. None if no estimates were provided yet.
    pub score: Option<u32>
}
