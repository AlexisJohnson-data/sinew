//! Claude through the user's own Claude Code install: Sinew drives the
//! unmodified `claude` binary in stream-json mode, the way the Agent SDK
//! does, and never handles Claude credentials itself.

mod cli;
mod model_info;
mod probe;
mod process;
mod provider;
mod transcript;

pub use cli::{claude_binary, start_login, status, ClaudeCodeStatus};
pub use model_info::{capabilities, MODEL_ID, MODEL_MAX_OUTPUT, MODEL_WINDOW};
pub use probe::{list_models, record_usage, refresh_plan_limits};
pub use process::{ClaudeProcess, ProcessSender};
pub use provider::{effort_flag, usage_from, ClaudeCodeProvider};
pub use transcript::render_transcript;
