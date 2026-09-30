//! `generate_doc_description` prompt against CPython (fixtures/describe.json, recorded by
//! gen/describe_fixture.py), plus its 400 handling.

use pi_llm::{Llm, LlmError, Message};
use pi_summary::describe::doc_description_prompt;
use pi_summary::{clean_structure_for_description, generate_doc_description};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/describe.json")).unwrap()
}

#[test]
fn prompt_matches_cpython() {
    let f = fixture();
    let clean = clean_structure_for_description(&f["structure"]);
    assert_eq!(clean, f["clean"]);
    assert_eq!(
        doc_description_prompt(&clean),
        f["prompt"].as_str().unwrap()
    );
}

struct Fails(u16);

#[async_trait::async_trait]
impl Llm for Fails {
    async fn complete(&self, _m: &[Message]) -> pi_llm::Result<String> {
        Err(LlmError::Status {
            status: self.0,
            message: String::new(),
        })
    }
}

#[tokio::test]
async fn context_overrun_yields_empty_description() {
    let clean = clean_structure_for_description(&fixture()["structure"]);
    assert_eq!(
        generate_doc_description(&clean, &Fails(400)).await.unwrap(),
        ""
    );
    assert!(generate_doc_description(&clean, &Fails(401)).await.is_err());
}
