pub mod bash;
pub mod edit;
pub mod read;
pub mod unix_kill;
pub mod write;

use std::path::Path;
use tau_agent::tool::AgentTool;

/// Create a set of coding tools for the given working directory.
pub fn create_coding_tools(cwd: &Path) -> Vec<AgentTool> {
    vec![
        read::create_tool(cwd),
        write::create_tool(cwd),
        edit::create_tool(cwd),
        bash::create_tool(cwd),
    ]
}
