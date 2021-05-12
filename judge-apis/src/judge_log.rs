//! Detailed information about run testing.
use serde::{Deserialize, Serialize};
pub use valuer_api::{JudgeLogKind, Status, StatusKind, SubtaskId};
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JudgeLogTestRow {
    pub test_id: pom::TestId,
    pub status: Option<Status>,
    pub test_stdin: Option<String>,
    pub test_stdout: Option<String>,
    pub test_stderr: Option<String>,
    pub test_answer: Option<String>,
    pub time_usage: Option<u64>,
    pub memory_usage: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JudgeLogSubtaskRow {
    pub subtask_id: SubtaskId,
    pub score: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JudgeLog {
    pub kind: JudgeLogKind,
    pub tests: Vec<JudgeLogTestRow>,
    pub subtasks: Vec<JudgeLogSubtaskRow>,
    pub compile_log: String,
    pub score: u32,
    pub is_full: bool,
    pub status: Status,
}

impl Default for JudgeLog {
    fn default() -> Self {
        Self {
            kind: JudgeLogKind::Contestant,
            tests: vec![],
            subtasks: vec![],
            compile_log: String::new(),
            score: 0,
            is_full: false,
            status: Status {
                code: "".to_string(),
                kind: StatusKind::NotSet,
            },
        }
    }
}
