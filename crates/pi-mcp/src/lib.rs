//! Retrieval tools as an MCP server: a port of the contract tools in
//! `pageindex/agent_tools.py` (local mode) served with `rmcp`.
pub mod consts;
pub mod pages;
pub mod pyval;
pub mod server;
pub mod structure;
pub mod surface;
pub mod tools;

pub use server::{PageIndexServer, ServerOptions, serve_stdio};
pub use surface::{
    AGENT_INSTRUCTIONS, local_description, local_schema, managed_instructions, tool_names,
};
pub use tools::call_tool;
