//! MCP round trip over an in-memory duplex pipe (the stdio transport's framing).

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ContentBlock};
use serde_json::{Value, json};

use pi_mcp::{PageIndexServer, ServerOptions, call_tool};
use pi_store::{DocStore, LocalApi};

fn seed(path: &std::path::Path) {
    let store = DocStore::new(path);
    let meta = pi_store::api::doc_meta(
        "pi-a",
        "report.pdf",
        Some("A test document"),
        "2026-08-01T10:00:00.123000",
        2,
        None,
        "flash",
    );
    let tree = json!([{"title": "Doc", "node_id": "0000", "start_index": 1, "end_index": 2}]);
    let pages = pi_store::api::pages_record(&["one", "two"]);
    store.save_document("pi-a", &meta, &tree, &pages).unwrap();
}

fn text_of(content: &[ContentBlock]) -> String {
    match &content[0] {
        ContentBlock::Text(t) => t.text.clone(),
        other => panic!("unexpected content {other:?}"),
    }
}

async fn roundtrip(include_management: bool) {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let server = PageIndexServer::new(
        LocalApi::new(tmp.path()),
        ServerOptions {
            include_management,
            doc_ids: None,
        },
    );
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let server_task = tokio::spawn(async move {
        let running = server.serve(tokio::io::split(server_io)).await.unwrap();
        running.waiting().await.unwrap();
    });
    let client = ().serve(tokio::io::split(client_io)).await.unwrap();

    let info = client.peer_info().unwrap();
    assert_eq!(
        info.instructions.as_deref(),
        Some(pi_mcp::AGENT_INSTRUCTIONS.as_str())
    );

    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(names, pi_mcp::tool_names(include_management));
    let page_tool = tools.iter().find(|t| t.name == "get_page_content").unwrap();
    assert_eq!(
        Value::Object((*page_tool.input_schema).clone()),
        pi_mcp::local_schema("get_page_content").unwrap()
    );
    assert_eq!(
        page_tool.annotations.as_ref().unwrap().read_only_hint,
        Some(true)
    );

    // Success and error envelopes pass through verbatim with isError.
    let args = json!({"doc_name": "report.pdf", "pages": "1-2"});
    let result = client
        .call_tool(
            CallToolRequestParams::new("get_page_content")
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    let expected = call_tool(
        &LocalApi::new(tmp.path()),
        "get_page_content",
        Some(&args),
        None,
    );
    assert_eq!(text_of(&result.content), expected.0);
    assert_eq!(result.is_error, Some(false));

    let bad = json!({"doc_name": "report.pdf", "pages": "abc"});
    let result = client
        .call_tool(
            CallToolRequestParams::new("get_page_content")
                .with_arguments(bad.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(text_of(&result.content).contains("INVALID_INPUT"));

    // remove_document is gated at registration.
    let remove = json!({"doc_names": ["report.pdf"]});
    let result = client
        .call_tool(
            CallToolRequestParams::new("remove_document")
                .with_arguments(remove.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    let payload: Value = serde_json::from_str(&text_of(&result.content)).unwrap();
    if include_management {
        assert_eq!(result.is_error, Some(false));
        assert_eq!(payload["results"][0]["status"], "deleted");
    } else {
        assert_eq!(result.is_error, Some(true));
        assert_eq!(payload["error"], "Unknown tool: remove_document");
        assert!(tmp.path().join("docs/pi-a/doc.json").is_file());
    }

    client.cancel().await.unwrap();
    server_task.await.unwrap();
}

#[tokio::test]
async fn stdio_roundtrip_read_tools() {
    roundtrip(false).await;
}

#[tokio::test]
async fn stdio_roundtrip_with_management() {
    roundtrip(true).await;
}
