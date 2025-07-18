use anyhow::Result;
use chonkier::types::{RecursiveChunk, RecursiveRules};
use chonkier::CharacterTokenizer;
use chonkier::RecursiveChunker;
use lazy_static::lazy_static;
use serde_json::{json, Value};
use std::env;
use std::path::Path;
use std::time::{Duration};
use tokio::runtime::Runtime;
use governor::{Quota, RateLimiter};
use std::num::NonZeroU32;
use governor::state::direct::NotKeyed;
use governor::state::InMemoryState;
use governor::clock::DefaultClock;


// Job type for embedding work
#[derive(Debug, Clone)]
pub struct EmbeddingJob {
    pub chunk: String,
    pub entity_id: String,
    pub port: u16,
}


// Global HTTP client with connection pooling
lazy_static! {
    static ref HTTP_CLIENT: reqwest::Client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(1000)
        .pool_idle_timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client");

    // Governor-based async rate limiter for embeddings (default: 1000 RPM)
    static ref EMBEDDING_LIMITER: RateLimiter<NotKeyed, InMemoryState, DefaultClock> =
        RateLimiter::direct(Quota::per_minute(NonZeroU32::new(1000).unwrap()));
}

// Chunk entity text
pub fn chunk_entity(text: &str) -> Result<Vec<String>> {
    let tokenizer = CharacterTokenizer::new();
    let chunker = RecursiveChunker::new(tokenizer, 2048, RecursiveRules::default());
    let chunks: Vec<RecursiveChunk> = chunker.chunk(&text.to_string());
    let chunks_str: Vec<String> = chunks.into_iter().map(|chunk| chunk.text).collect();
    Ok(chunks_str)
}

// Async version of embed_entity with rate limiting
pub async fn embed_entity_async(text: String) -> Result<Vec<f64>> {
    // Handle empty text case to avoid API errors
    if text.trim().is_empty() {
        return Err(anyhow::anyhow!("Cannot embed empty text"));
    }

    // Governor async rate limiting
    EMBEDDING_LIMITER.until_ready().await;

    // Use gemini api to embed text with the global HTTP client
    let api_key = match env::var("GEMINI_API_KEY") {
        Ok(key) => key,
        Err(_) => return Err(anyhow::anyhow!("GEMINI_API_KEY environment variable not set"))
    };

    let res = HTTP_CLIENT.post("https://generativelanguage.googleapis.com/v1beta/models/text-embedding-004:embedContent")
        .header("x-goog-api-key", api_key)
        .header("Content-Type", "application/json")
        .json(&json!({
            "model": "models/text-embedding-004",
            "content": {
                "parts": [{
                    "text": text,
                }]
            }
        }))
        .send()
        .await?;

    // Check response status
    if !res.status().is_success() {
        let status = res.status();
        let error_text = res.text().await.unwrap_or_else(|_| "<could not read response body>".to_string());
        return Err(anyhow::anyhow!("API returned error status {}: {}", status, error_text));
    }

    let body = res.json::<Value>().await?;
    
    // More detailed error handling for the response format
    if !body.is_object() {
        return Err(anyhow::anyhow!("API response is not a JSON object: {:?}", body));
    }
    
    if !body.get("embedding").is_some() {
        return Err(anyhow::anyhow!("API response missing 'embedding' field: {:?}", body));
    }
    
    let embedding = body["embedding"]["values"].as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid embedding response format, missing 'values' array: {:?}", body))?;

    // Convert values to f64, with better error handling
    let mut result = Vec::with_capacity(embedding.len());
    for (i, v) in embedding.iter().enumerate() {
        match v.as_f64() {
            Some(val) => result.push(val),
            None => return Err(anyhow::anyhow!("Non-numeric value at position {} in embedding: {:?}", i, v))
        }
    }

    Ok(result)
}

// Send POST request to Helix instance
pub fn post_request(url: &str, body: Value, runtime: &Runtime) -> Result<Value> {
    // Execute the async operation in the shared runtime
    runtime.block_on(async {
        post_request_async(url, body).await
    })
}

// Async version of post_request
pub async fn post_request_async(url: &str, body: Value) -> Result<Value> {
    // Use the global HTTP client with connection pooling
    let res = match HTTP_CLIENT.post(url).json(&body).send().await {
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

    Ok(res.json::<Value>().await?)
}

// Get language from file extension
pub fn get_language(file_path: &Path) -> Option<tree_sitter::Language> {
    let extension = file_path.extension().and_then(|s| s.to_str());
    match extension {
        Some("py") => Some(tree_sitter_python::LANGUAGE.into()),
        Some("rs") => Some(tree_sitter_rust::LANGUAGE.into()),
        Some("zig") => Some(tree_sitter_zig::LANGUAGE.into()),
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
