use anyhow::Result;
use chonkier::types::{RecursiveChunk, RecursiveRules};
use chonkier::CharacterTokenizer;
use chonkier::RecursiveChunker;
use lazy_static::lazy_static;
use serde_json::{json, Value};
use std::env;
use std::path::Path;
use std::time::{Duration};
use governor::{Quota, RateLimiter};
use std::num::NonZeroU32;
use governor::state::direct::NotKeyed;
use governor::state::InMemoryState;
use governor::clock::DefaultClock;
use std::sync::{
    atomic::{AtomicUsize},
};
use std::collections::HashMap;
use tokio::task::JoinHandle;
use crate::queries::{get_sub_folders, get_folder_files};
use async_recursion::async_recursion;

// Global counter to track total number of chunks processed
pub static TOTAL_CHUNKS: AtomicUsize = AtomicUsize::new(0);
pub static PENDING_EMBEDDINGS: AtomicUsize = AtomicUsize::new(0);
pub static COMPLETED_EMBEDDINGS: AtomicUsize = AtomicUsize::new(0);

// Job type for embedding work
#[derive(Debug, Clone)]
pub struct EmbeddingJob {
    pub chunk: String,
    pub entity_id: String,
    pub port: u16,
}

// Global HTTP client with connection pooling
lazy_static! {
    static ref embedding_client: reqwest::Client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(3000)
        .pool_idle_timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client");

    static ref helix_client: reqwest::Client = reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(500)
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .expect("Failed to create HTTP client");

    static ref EMBEDDING_LIMITER: RateLimiter<NotKeyed, InMemoryState, DefaultClock> =
        RateLimiter::direct(Quota::per_minute(NonZeroU32::new(4000).unwrap()));

    static ref HELIX_LIMITER: RateLimiter<NotKeyed, InMemoryState, DefaultClock> =
        RateLimiter::direct(Quota::per_second(NonZeroU32::new(100).unwrap()));
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

    EMBEDDING_LIMITER.until_ready().await;

    // Use gemini api to embed text with the global HTTP client
    let api_key = match env::var("GEMINI_API_KEY") {
        Ok(key) => key,
        Err(_) => return Err(anyhow::anyhow!("GEMINI_API_KEY environment variable not set"))
    };

    let res = embedding_client.post("https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-001:embedContent")
        .header("x-goog-api-key", api_key)
        .header("Content-Type", "application/json")
        .json(&json!({
            "model": "models/gemini-embedding-001",
            "content": {
                "parts": [{
                    "text": text,
                }]
            },
            "task_type": "SEMANTIC_SIMILARITY"
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

// Async version of post_request
pub async fn post_request_async(url: &str, body: Value) -> Result<Value> {
    HELIX_LIMITER.until_ready().await;

    // Use the global HTTP client with connection pooling
    let res = match helix_client.post(url).json(&body).send().await {
        Ok(response) => response,
        Err(e) => {
            if e.is_timeout() {
                println!("Request timed out. Check if the server is running and responding.");
            } else if e.is_connect() {
                println!("Connection failed. Make sure the server is running at {}",url);
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
        Some("cpp") | Some("cc") | Some("cxx") => Some(tree_sitter_cpp::LANGUAGE.into()),
        Some("c") | Some("h") => Some(tree_sitter_c::LANGUAGE.into()),
        Some("ts") | Some("mts") | Some("cts")=> Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        Some("tsx") => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        Some("js") | Some("jsx") | Some("mjs") | Some("mjsx") | Some("cjs") | Some("cjsx") => Some(tree_sitter_javascript::LANGUAGE.into()),
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

// Shared delete functions used by both ingestion and update processes

#[async_recursion]
pub async fn delete_folder(
    folder_id: String,
    port: u16,
) -> Result<()> {
    let subfolder_name_ids  = get_sub_folders(folder_id.clone(), port).await?;
    // println!("Subfolder IDs: {:#?}", subfolder_name_ids);

    // Find folders that are not in the index
    let unseen_folders = subfolder_name_ids.keys().cloned().collect::<Vec<_>>();

    let tasks: Vec<JoinHandle<Result<()>>> = unseen_folders.into_iter().map(|folder_name| {
        let sub_folder_id = subfolder_name_ids.get(&folder_name).unwrap().to_string().clone();

        tokio::spawn(Box::pin(async move {
            delete_folder(sub_folder_id, port).await
        }))
    }).collect();

    for task in tasks {
        task.await??;
    }

    // Get folder files
    let folder_file_name_ids = get_folder_files(folder_id.clone(), port).await?;
    // println!("Subfolder file IDs: {:#?}", folder_file_name_ids);

    let unseen_files = folder_file_name_ids.keys().cloned().collect::<Vec<_>>();

    delete_files(unseen_files.clone(), folder_file_name_ids.clone(), port).await?;

    let url = format!("http://localhost:{}/{}", port, "deleteFolder");
    let payload = json!({ "folder_id": folder_id });
    post_request_async(&url, payload).await?;
    Ok(())
}

pub async fn delete_files(
    unseen_files: Vec<String>,
    file_name_ids: HashMap<String, (String, String)>,
    port: u16
) -> Result<()> {
    let tasks: Vec<JoinHandle<Result<()>>> = unseen_files.into_iter().map(|file_name| {
        let file_id = file_name_ids.get(&file_name).unwrap().0.clone();

        tokio::spawn(async move {
            let url = format!("http://localhost:{}/{}", port, "deleteFile");
            let payload = json!({ "file_id": file_id });
            let _ = post_request_async(&url, payload).await;
            
            let _ = delete_entities(file_id, true, port).await;
            Ok(())
        })
    }).collect();

    for task in tasks {
        task.await??;
    }
    Ok(())
}

#[async_recursion]
pub async fn delete_entities(
    parent_id: String,
    is_super: bool,
    port: u16,
) -> Result<()> {
    let url;
    let body;
    let res_index;
    let delete_url;
    if is_super {
        url = format!("http://localhost:{}/{}", port, "getFileEntities");
        body = json!({ "file_id": parent_id });
        res_index = "entity";
        delete_url = format!("http://localhost:{}/{}", port, "deleteSuperEntity");
    } else {
        url = format!("http://localhost:{}/{}", port, "getSubEntities");
        body = json!({ "entity_id": parent_id });
        res_index = "entities";
        delete_url = format!("http://localhost:{}/{}", port, "deleteSubEntity");
    }

    let response = post_request_async(&url, body).await?;
    let entities = response
        .get(res_index)
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("Entities not found"))?;

    let entity_ids: Vec<String> = entities
        .iter()
        .map(|v| v.get("id").and_then(|v| v.as_str()).unwrap().to_string())
        .collect();

    let tasks: Vec<JoinHandle<Result<()>>> = entity_ids.into_iter().map(|entity_id| {
        let delete_url_clone = delete_url.clone();
        tokio::spawn(Box::pin(async move {
            let _ = delete_entities(entity_id.clone(), false, port).await;
            let _ = post_request_async(&delete_url_clone, json!({ "entity_id": entity_id })).await;
            Ok(())
        }))
    }).collect();

    for task in tasks {
        task.await??;
    }

    Ok(())
}
