//! The local tool surface: names, descriptions, schemas and instructions
//! (`tool_names`, `_local_description`, `_local_schema`, `AGENT_INSTRUCTIONS`).

use std::sync::LazyLock;

use serde_json::Value;

use crate::consts::{
    AGENT_INSTRUCTIONS_PARTS, CHAT_HEADER, LOCAL_BROWSE_DESCRIPTION, LOCAL_DOC_NAME_DESCRIPTION,
    LOCAL_DOC_NAMES_DESCRIPTION, LOCAL_HIDDEN_PARAMS, LOCAL_IMAGE_SENTENCE_RE, MANAGEMENT_TOOLS,
    READ_TOOLS, tool_contract,
};

/// `AGENT_INSTRUCTIONS`: the built-in local-subset agent guidance (the system prompt the
/// SDK serves for local libraries). // ref: pageindex/agent_tools.py:1619
pub static AGENT_INSTRUCTIONS: LazyLock<String> =
    LazyLock::new(|| AGENT_INSTRUCTIONS_PARTS.join("\n\n"));

/// `CHAT_HEADER + "\n\n" + AGENT_INSTRUCTIONS`: the managed chat's system prompt without
/// caller additions. // ref: pageindex/local_chat.py:28-33
pub fn managed_instructions() -> String {
    format!("{CHAT_HEADER}\n\n{}", *AGENT_INSTRUCTIONS)
}

/// `tool_names(include_management)`. // ref: pageindex/agent_tools.py:1189
pub fn tool_names(include_management: bool) -> Vec<&'static str> {
    let mut names = READ_TOOLS.to_vec();
    if include_management {
        names.extend(MANAGEMENT_TOOLS);
    }
    names
}

/// `_local_description(name)`. // ref: pageindex/agent_tools.py:1316
pub fn local_description(name: &str) -> Option<String> {
    let contract = tool_contract();
    let base = contract
        .get(name)?
        .get("description")?
        .as_str()?
        .to_string();
    Some(match name {
        "browse_documents" => LOCAL_BROWSE_DESCRIPTION.to_string(),
        "get_page_content" => regex::Regex::new(LOCAL_IMAGE_SENTENCE_RE)
            .expect("valid pattern")
            .replace_all(&base, "")
            .into_owned(),
        _ => base,
    })
}

/// `_local_schema(name)`: the contract schema minus cloud-only parameters, with local
/// parameter descriptions. // ref: pageindex/agent_tools.py:1320
pub fn local_schema(name: &str) -> Option<Value> {
    let mut schema = tool_contract().get(name)?.get("schema")?.clone();
    let props = schema.get_mut("properties")?.as_object_mut()?;
    if let Some((_, hidden)) = LOCAL_HIDDEN_PARAMS.iter().find(|(n, _)| *n == name) {
        for param in *hidden {
            props.shift_remove(*param);
        }
    }
    let local_desc = match name {
        "get_document" | "get_document_structure" | "get_page_content" => {
            Some(("doc_name", LOCAL_DOC_NAME_DESCRIPTION))
        }
        "remove_document" => Some(("doc_names", LOCAL_DOC_NAMES_DESCRIPTION)),
        _ => None,
    };
    if let Some((param, text)) = local_desc
        && let Some(Value::Object(spec)) = props.get_mut(param)
    {
        spec.insert("description".into(), Value::String(text.into()));
    }
    Some(schema)
}

/// The contract annotations of a tool.
pub fn annotations(name: &str) -> Option<Value> {
    tool_contract().get(name)?.get("annotations").cloned()
}
