//! Document description. ref: pageindex/utils.py:1102-1140

use pi_llm::{Llm, LlmError, complete_prompt};
use serde_json::{Map, Value};

use crate::prompts;
use crate::pyrepr::repr;
use pi_optimize::expand::truthy;

/// `create_clean_structure_for_description(structure)`: keep `title`, `node_id`, `summary`,
/// `prefix_summary` and non-empty `nodes`. ref: utils.py:1102
pub fn clean_structure_for_description(structure: &Value) -> Value {
    match structure {
        Value::Object(m) => {
            let mut clean = Map::new();
            for key in ["title", "node_id", "summary", "prefix_summary"] {
                if let Some(v) = m.get(key) {
                    clean.insert(key.into(), v.clone());
                }
            }
            if let Some(nodes) = m.get("nodes")
                && truthy(nodes)
            {
                clean.insert("nodes".into(), clean_structure_for_description(nodes));
            }
            Value::Object(clean)
        }
        Value::Array(a) => Value::Array(a.iter().map(clean_structure_for_description).collect()),
        other => other.clone(),
    }
}

/// The prompt `generate_doc_description` sends (`{structure}` is Python's `repr`).
pub fn doc_description_prompt(clean_structure: &Value) -> String {
    prompts::doc_description(&repr(clean_structure))
}

/// `generate_doc_description(structure, model)`: a one-sentence description; a per-prompt 400
/// (context overrun) yields `""`. ref: utils.py:1125
pub async fn generate_doc_description(
    clean_structure: &Value,
    llm: &dyn Llm,
) -> Result<String, LlmError> {
    match complete_prompt(llm, &doc_description_prompt(clean_structure)).await {
        Ok(s) => Ok(s),
        Err(e) if e.status_code() == Some(400) => Ok(String::new()),
        Err(e) => Err(e),
    }
}
