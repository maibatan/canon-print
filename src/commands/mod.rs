pub mod diagnose;
pub mod inspect;
pub mod list;
pub mod print;
pub mod status;

use serde_json::Value;

/// Result of a CLI command: machine-readable JSON, human text and the exit code.
pub struct Outcome {
    pub json: Value,
    pub text: String,
    pub exit_code: i32,
}
