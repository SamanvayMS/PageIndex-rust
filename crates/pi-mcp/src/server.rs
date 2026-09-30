//! MCP server over the contract tools (`rmcp`): stdio, and streamable HTTP behind the
//! `http` feature. Mirrors the SDK's in-process server (`as_claude_mcp` in local mode):
//! read tools always, `remove_document` only with `include_management`, each result one
//! text block carrying the JSON envelope, `isError` set on error envelopes.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Value, json};

use crate::surface::{
    AGENT_INSTRUCTIONS, annotations, local_description, local_schema, tool_names,
};
use crate::tools::call_tool;
use pi_store::LocalApi;

/// Server options.
#[derive(Debug, Clone, Default)]
pub struct ServerOptions {
    /// Register `remove_document` (destructive) as well as the read tools.
    pub include_management: bool,
    /// Restrict every document lookup to these ids (the SDK's `doc_id` chat scope).
    pub doc_ids: Option<Vec<String>>,
}

/// The MCP handler: one local store, the gated tool set.
#[derive(Debug, Clone)]
pub struct PageIndexServer {
    api: Arc<LocalApi>,
    options: Arc<ServerOptions>,
}

impl PageIndexServer {
    pub fn new(api: LocalApi, options: ServerOptions) -> Self {
        PageIndexServer {
            api: Arc::new(api),
            options: Arc::new(options),
        }
    }

    /// The registered tools, as `tools/list` returns them.
    pub fn tools(&self) -> Vec<Tool> {
        tool_names(self.options.include_management)
            .into_iter()
            .map(|name| {
                let schema = match local_schema(name) {
                    Some(Value::Object(m)) => m,
                    _ => serde_json::Map::new(),
                };
                let mut tool = Tool::new(
                    name,
                    local_description(name).unwrap_or_default(),
                    Arc::new(schema),
                );
                if let Some(a) = annotations(name)
                    .and_then(|a| serde_json::from_value::<ToolAnnotations>(a).ok())
                {
                    tool = tool.with_annotations(a);
                }
                tool
            })
            .collect()
    }

    /// Run one tool call: the envelope text and its error flag. A tool outside the gated
    /// set answers with the unknown-tool envelope.
    pub fn run(&self, name: &str, arguments: Option<Value>) -> (String, bool) {
        let registered = tool_names(self.options.include_management);
        if !registered.contains(&name) {
            return call_tool_gated(name, &registered);
        }
        call_tool(
            &self.api,
            name,
            arguments.as_ref(),
            self.options.doc_ids.as_deref(),
        )
    }
}

/// The unknown-tool envelope for a tool the server does not register.
fn call_tool_gated(name: &str, registered: &[&str]) -> (String, bool) {
    let payload = json!({
        "error": format!("Unknown tool: {name}"),
        "errorCode": "INVALID_INPUT",
        "tool_name": name,
        "available_tools": registered,
        "next_steps": {"summary": "Tool not found",
                       "options": [format!("Available tools: {}", registered.join(", "))]},
    });
    (pi_store::pyjson::dumps(&payload), true)
}

impl ServerHandler for PageIndexServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("pageindex", env!("CARGO_PKG_VERSION")))
            .with_instructions(AGENT_INSTRUCTIONS.as_str())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let this = self.clone();
        let name = request.name.to_string();
        let arguments = request.arguments.map(Value::Object);
        // Store reads (and the rare completion wait) block; keep them off the reactor.
        let (text, is_error) = tokio::task::spawn_blocking(move || this.run(&name, arguments))
            .await
            .unwrap_or_else(|e| {
                let payload = json!({
                    "error": format!("tool failed: {e}"),
                    "errorCode": "INTERNAL_ERROR",
                    "next_steps": {"summary": "Unexpected error while running the tool",
                                   "options": ["Try the request again"]},
                });
                (pi_store::pyjson::dumps(&payload), true)
            });
        let content = vec![ContentBlock::text(text)];
        let result = if is_error {
            CallToolResult::error(content)
        } else {
            CallToolResult::success(content)
        };
        Ok(result.into())
    }
}

/// Serve over stdio until the client disconnects.
pub async fn serve_stdio(server: PageIndexServer) -> std::io::Result<()> {
    let running = server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(std::io::Error::other)?;
    running.waiting().await.map_err(std::io::Error::other)?;
    Ok(())
}

/// Serve streamable HTTP at `http://<addr>/mcp` until the process ends.
#[cfg(feature = "http")]
pub async fn serve_http(
    server: PageIndexServer,
    addr: std::net::SocketAddr,
) -> std::io::Result<()> {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await
}
