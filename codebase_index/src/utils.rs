use anyhow::Result;
use chonkier::types::{RecursiveChunk, RecursiveRules};
use chonkier::CharacterTokenizer;
use chonkier::RecursiveChunker;
use serde_json::{json, Value};
use std::env;
use std::path::Path;

// Chunk entity text
pub fn chunk_entity(text: &str) -> Result<Vec<String>> {
    let tokenizer = CharacterTokenizer::new();
    let chunker = RecursiveChunker::new(tokenizer, 2048, RecursiveRules::default());
    let chunks: Vec<RecursiveChunk> = chunker.chunk(&text.to_string());
    let chunks_str: Vec<String> = chunks.into_iter().map(|chunk| chunk.text).collect();
    Ok(chunks_str)
}

// Embed entity text
pub fn embed_entity(text: String) -> Result<Vec<f64>> {
    // use gemini api to embed text
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(1))
        .build()?;
    let res = client.post("https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-exp-03-07:embedContent")
        .header("x-goog-api-key", env::var("GEMINI_API_KEY").unwrap())
        .header("Content-Type", "application/json")
        .json(&json!({
            "model": "models/gemini-embedding-exp-03-07",
            "content": {
                "parts": [{
                    "text": text,
                }]
            }
        }))
        .send()
        .unwrap();

    let body = res.json::<Value>().unwrap();
    let embedding = body["embedding"]["values"].as_array().unwrap();

    Ok(embedding
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect::<Vec<f64>>())
}

// Send POST request to Helix instance
pub fn post_request(url: &str, body: Value) -> Result<Value> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(1))
        .build()?;

    let res = match client.post(url).json(&body).send() {
        Ok(response) => response,
        Err(e) => {
            if e.is_timeout() {
                println!("Request timed out. Check if the server is running and responding.");
            } else if e.is_connect() {
                println!(
                    "Connection failed. Make sure the server is running at {}",
                    url
                );
            }
            return Err(anyhow::anyhow!("HTTP request failed: {}", e));
        }
    };

    Ok(res.json::<Value>()?)
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
#[derive(Clone)]
pub struct CodeEntity {
    pub entity_type: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub order: usize,
    pub text: String,
}
