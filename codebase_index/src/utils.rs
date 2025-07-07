use anyhow::Result;
use serde_json::Value;
use std::path::Path;
use tree_sitter::{Node};

// Chunk entity text
// TODO: Replace with actual chunking function
pub async fn chunk_entity(text: &str) -> Vec<&str> {
    text.as_bytes()
        .chunks(1000)
        .map(std::str::from_utf8)
        .collect::<Result<Vec<&str>, _>>()
        .unwrap_or_else(|_| vec![text]) // Fallback to whole text on error
}

// Embed entity text
// TODO: Replace with actual embedding function
pub async fn embed_entity(_text: &str) -> Vec<f64> {
    vec![0.1; 768]
}

// Send POST request to Helix instance
pub async fn post_request(url: &str, body: Value) -> Result<Value> {
    let client = reqwest::Client::new();
    let res = client.post(url).json(&body).send().await?;

    if res.status().is_success() {
        let json_res = res.json::<Value>().await?;
        Ok(json_res)
    } else {
        let status = res.status();
        let error_body = res.text().await?;
        Err(anyhow::anyhow!(
            "API request failed with status {}: {}",
            status,
            error_body
        ))
    }
}

// Get language from file extension
pub fn get_language(file_path: &Path) -> Option<tree_sitter::Language> {
    let extension = file_path.extension().and_then(|s| s.to_str());
    match extension {
        Some("py") => Some(tree_sitter_python::language()),
        Some("js") => Some(tree_sitter_javascript::language()),
        _ => None,
    }
}

// Code entity struct
#[derive(Debug)]
pub struct CodeEntity {
    pub entity_type: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub order: usize,
    pub text: String,
    pub children: Vec<CodeEntity>,
}

// Extract code entities recursively
pub fn extract_entities_recursive(node: Node, source_code: &str) -> Vec<CodeEntity> {
    let mut entities = Vec::new();
    let mut order = 1;
    for child in node.children(&mut node.walk()) {
        let grandchildren = extract_entities_recursive(child, source_code);
        entities.push(CodeEntity {
            entity_type: child.kind().to_string(),
            start_byte: child.start_byte(),
            end_byte: child.end_byte(),
            order,
            text: source_code[child.start_byte()..child.end_byte()].to_string(),
            children: grandchildren,
        });

        order += 1;
    }
    entities
}