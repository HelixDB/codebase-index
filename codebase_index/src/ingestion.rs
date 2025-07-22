use anyhow::Result;
use ignore::WalkBuilder;
use futures::future::join_all;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc
};
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;
use crate::utils::CodeEntity;
use tree_sitter::{Node, Parser};
#[derive(Clone)]
pub struct OwnedNode {
    kind: String,
    start_byte: usize,
    end_byte: usize,
    text: String,
    children: Vec<OwnedNode>,
}

// Import from our modules
use crate::utils::{
    post_request_async, chunk_entity, get_language, EmbeddingJob, TOTAL_CHUNKS
};

// Add use async_recursion::async_recursion;
use async_recursion::async_recursion;

pub async fn ingestion(
    root_path: PathBuf,
    port: u16,
    tx: Sender<EmbeddingJob>,
) -> Result<String> {
    println!("Starting ingestion for directory: {}", root_path.display());

    // Create a root entry in the index
    let root_name = root_path.file_name().unwrap().to_str().unwrap();
    let url = format!("http://localhost:{}/{}", port, "createRoot");
    let root_response = post_request_async(&url, json!({ "name": root_name })).await?;
    let root_id = root_response
        .get("root")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Root ID not found"))?;

    println!("\nRoot created");

    // Load index types
    let index_types = fs::read_to_string("index-types.json")?;
    let index_types: serde_json::Value = serde_json::from_str(&index_types)?;
    let index_types = Arc::new(index_types);

    let file_types = fs::read_to_string("file_types.json")?;
    let file_types: serde_json::Value = serde_json::from_str(&file_types)?;
    let file_types = Arc::new(file_types);

    // Start populating the index with directory contents
    populate(
        root_path,root_id.to_string(),port,
        true,index_types,file_types,tx
    ).await?;

    Ok(root_id.to_string())
}

/// Recursively populates the index with directory contents
#[async_recursion]
pub async fn populate(
    current_path: PathBuf,
    parent_id: String,
    port: u16,
    is_super: bool,
    index_types: Arc<serde_json::Value>,
    file_types: Arc<serde_json::Value>,
    tx: Sender<EmbeddingJob>,
) -> Result<()> {
    // Initialize walker builder
    let mut walker_builder = WalkBuilder::new(&current_path);
    walker_builder.max_depth(Some(1));

    // Add default ignore patterns
    for pattern in &[".git/"] {
        walker_builder.add_ignore(pattern);
    }

    // Collect entries to process
    let entries: Vec<_> = walker_builder.build()
        .filter_map(|result| result.ok())
        .filter(|entry| entry.path() != current_path)
        .collect();

    // Process entries concurrently
    let tasks: Vec<JoinHandle<Result<()>>> = entries.into_iter().map(|entry| {
        let path_buf = entry.path().to_path_buf();
        let parent_id_clone = parent_id.clone();
        let index_types_clone = index_types.clone();
        let file_types_clone = file_types.clone();
        let tx_clone = tx.clone();

        tokio::spawn(async move {
            if path_buf.is_dir() {
                // Get folder information
                let folder_name = path_buf.file_name().unwrap().to_str().unwrap();
                let endpoint = if is_super {"createSuperFolder"} else {"createSubFolder"};
                let url = format!("http://localhost:{}/{}", port, endpoint);
                let payload = if is_super {
                    json!({ "name": folder_name, "root_id": parent_id_clone })
                } else {
                    json!({ "name": folder_name, "folder_id": parent_id_clone })
                };

                // Send request to create folder and get its ID
                println!("\nSubmitting {} folder for processing", folder_name);
                match post_request_async(&url, payload).await {
                    Ok(res) => {
                        if let Some(folder_id) = res
                            .get(if is_super { "folder" } else { "subfolder" })
                            .and_then(|v| v.get("id"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                        {
                            let path_buf_clone = path_buf.clone();
                            if let Err(e) = Box::pin(populate(
                                path_buf_clone,folder_id,port,
                                false,index_types_clone, file_types_clone, tx_clone
                            )).await {
                                eprintln!("Error populating folder {}: {}",folder_name, e);
                            }
                            Ok(())
                        } else {
                            eprintln!("Failed to extract folder ID from response for: {}", folder_name);
                            Ok(())
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to create folder {}: {}", folder_name, e);
                        Ok(())
                    }
                }
            } else if path_buf.is_file() {
                process_file(
                    path_buf,parent_id_clone,is_super,
                    port, index_types_clone,file_types_clone,tx_clone
                ).await
            } else {
                Ok(())
            }
        })
    }).collect();

    for task in tasks {
        task.await??;
    }

    Ok(())
}

/// Processes a single file and extracts entities
pub async fn process_file(
    file_path: PathBuf,
    parent_id: String,
    is_super: bool,
    port: u16,
    index_types: Arc<serde_json::Value>,
    file_types: Arc<serde_json::Value>,
    tx: Sender<EmbeddingJob>,
) -> Result<()> {
    // Read file contents
    let source_code = match fs::read_to_string(&file_path) {
        Ok(source_code) => source_code,
        Err(e) => {
            eprintln!("Skipped {}: {}", file_path.file_name().unwrap().to_str().unwrap(), e);
            return Ok(());
        }
    };

    let file_name = file_path.file_name().unwrap().to_str().unwrap();
    let extension = file_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("txt");

    let supported = file_types.get("supported").unwrap().as_array().unwrap();
    let unsupported = file_types.get("unsupported").unwrap().as_array().unwrap();

    // Parse file with Tree Sitter
    if let Some(language) = get_language(&file_path) {
        // Parse file
        let mut parser = Parser::new();
        parser.set_language(&language)?;
        let tree = parser.parse(&source_code, None).unwrap();

        // Create file
        let file_type = if is_super { "super" } else { "sub" };
        let endpoint = if is_super {"createSuperFile"} else {"createFile"};
        let url = format!("http://localhost:{}/{}", port, endpoint);
        let payload = if is_super {
            json!({ "name": file_name, "extension": extension, "root_id": parent_id, "text": source_code })
        } else {
            json!({ "name": file_name, "extension": extension, "folder_id": parent_id, "text": source_code })
        };

        // Send request to create file
        println!("\nProcessing {} file: {}", file_type, file_name);
        let file_response = match post_request_async(&url, payload).await {
            Ok(response) => response,
            Err(e) => {
                eprintln!("Failed to create file {}: {}", file_name, e);
                return Err(anyhow::anyhow!("Failed to create file: {}", e));
            }
        };

        if !supported.iter().any(|v| v.as_str().map_or(false, |s| s == extension || s == "ALL")){
            println!("File {} is skipped", file_name);
            return Ok(());
        }

        let file_id = file_response
            .get("file")
            .and_then(|v| v.get("id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                eprintln!("Failed to extract file ID from response for: {}", file_name);
                anyhow::anyhow!("File ID not found in response")
            })?;

        // Process entities
        let root_node = tree.root_node();
        let owned_nodes = build_owned_nodes(root_node, &source_code);
        ingest_entities(owned_nodes, file_id.to_string(), port, extension.to_string(), index_types, tx).await?;
    } else {
        // Create file without entities
        let endpoint = if is_super {"createSuperFile"} else {"createFile"};
        let url = format!("http://localhost:{}/{}", port, endpoint);
        let payload = if is_super {
            json!({ "name": file_name, "extension": extension, "root_id": parent_id, "text": source_code })
        } else {
            json!({ "name": file_name, "extension": extension, "folder_id": parent_id, "text": source_code })
        };

        // Send request to create file
        println!("\nProcessing unsupported file: {}", file_name);
        let response = post_request_async(&url, payload).await?;

        if !unsupported.iter().any(|v| v.as_str().map_or(false, |s| s == extension || s == "ALL")){
            println!("File {} is skipped", file_name);
            return Ok(());
        }

        let file_id = response.get("file").and_then(|v| v.get("id")).and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("File ID not found"))?;

        let chunks = chunk_entity(&source_code).unwrap();
        let order_counter = Arc::new(AtomicUsize::new(1));
        TOTAL_CHUNKS.fetch_add(chunks.len(), Ordering::SeqCst);

        process_unsupported_file(chunks, file_id.to_string(), port, order_counter, tx).await?;
    }
    Ok(())
}

pub async fn process_unsupported_file(
    chunks: Vec<String>,
    file_id: String,
    port: u16,
    order_counter: Arc<AtomicUsize>,
    tx: Sender<EmbeddingJob>,
) -> Result<()> {
    let tasks: Vec<JoinHandle<()>> = chunks.into_iter().map(|chunk| {
        let file_id_clone = file_id.clone();
        let order_counter_clone = order_counter.clone();
        let tx_clone = tx.clone();

        tokio::spawn(async move {
            let url = format!("http://localhost:{}/{}", port, "createSuperEntity");
            let payload = json!({
                    "file_id": file_id_clone,
                    "entity_type": "chunk",
                    "text": chunk,
                    "start_byte": 0,
                    "end_byte": chunk.len() as i64,
                    "order": order_counter_clone.fetch_add(1, Ordering::SeqCst),
                });

            // Send request to create entity
            let entity_response = post_request_async(&url, payload).await;
            let entity_id = match entity_response {
                Ok(response) => response.get("entity")
                    .and_then(|v| v.get("id"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                Err(e) => {
                    eprintln!("Failed to create entity: {}", e);
                    None
                }
            };
            
            // Generate embedding
            if let Some(entity_id) = entity_id {
                let job = EmbeddingJob {chunk: chunk.clone(), entity_id, port};
                match tx_clone.try_send(job) {
                    Ok(_) => {},
                    Err(err) => {
                        // Channel is full; send asynchronously
                        let job_back = err.into_inner();
                        let tx_async = tx_clone.clone();
                        tokio::spawn(async move {
                            if let Err(e) = tx_async.send(job_back).await {
                                eprintln!("Failed to send embedding job asynchronously: {}", e);
                            }
                        });
                    }
                }
            }
        })
    }).collect();

    join_all(tasks).await;

    Ok(())
}

pub async fn ingest_entities(
    owned_nodes: Vec<OwnedNode>,
    file_id: String,
    port: u16,
    extension: String,
    index_types: Arc<serde_json::Value>,
    tx: Sender<EmbeddingJob>,
) -> Result<()> {
    let order_counter = Arc::new(AtomicUsize::new(1));
    let tasks: Vec<JoinHandle<Result<()>>> = owned_nodes.into_iter().map(|owned| {
        let file_id_clone = file_id.clone();
        let extension_clone = extension.clone();
        let index_types_clone = index_types.clone();
        let tx_clone = tx.clone();
        let order_counter_clone = order_counter.clone();
        tokio::spawn(async move {
            let current_order = order_counter_clone.fetch_add(1, Ordering::SeqCst);
            // Get index_types for file extension
            if let Some(types) = index_types_clone.get(&extension_clone) {
                if let Some(types_array) = types.as_array() {
                    // Check if ALL is not in index_types
                    if types_array.iter().any(|v| v.as_str().map_or(false, |s| s != "ALL")) {
                        // Super entity type in index_types
                        if types_array.iter().any(|v| v.as_str().map_or(false, |s| s == owned.kind)){
                            // Has super content
                            let entity_start_byte = owned.start_byte;
                            let entity_end_byte = owned.end_byte;
                            let entity_content = &owned.text;
                            let url = format!("http://localhost:{}/{}", port, "createSuperEntity");
                            let payload = json!({
                                "file_id": file_id_clone.clone(),
                                "entity_type": owned.kind,
                                "text": entity_content,
                                "start_byte": entity_start_byte,
                                "end_byte": entity_end_byte,
                                "order": current_order,
                            });
                            // Send request
                            if let Ok(entity_response) = post_request_async(&url, payload).await {
                                if let Some(entity_id) = entity_response.get("entity").and_then(|v| v.get("id")).and_then(|v| v.as_str()) {
                                    if let Ok(chunks) = chunk_entity(entity_content) {
                                        TOTAL_CHUNKS.fetch_add(chunks.len(), Ordering::SeqCst);
                                        let chunk_tasks: Vec<JoinHandle<()>> = chunks.into_iter().map(|chunk| {
                                            let chunk_clone = chunk.clone();
                                            let entity_id_clone = entity_id.to_string();
                                            let tx_clone_inner = tx_clone.clone();
                                            tokio::spawn(async move {
                                                let job = EmbeddingJob {chunk: chunk_clone, entity_id: entity_id_clone, port};
                                                match tx_clone_inner.try_send(job) {
                                                    Ok(_) => {},
                                                    Err(err) => {
                                                        let job_back = err.into_inner();
                                                        let tx_async = tx_clone_inner.clone();
                                                        tokio::spawn(async move {
                                                            if let Err(e) = tx_async.send(job_back).await {
                                                                eprintln!("Failed to send embedding job asynchronously: {}", e);
                                                            }
                                                        });
                                                    }
                                                }
                                            })
                                        }).collect();
                                        join_all(chunk_tasks).await;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            process_entity(owned, file_id_clone, port, true, current_order, extension_clone, index_types_clone, tx_clone).await
        })
    }).collect();
    for task in tasks {
        task.await??;
    }
    Ok(())
}

/// Processes an entity and its children recursively
#[async_recursion]
async fn process_entity(
    owned: OwnedNode,
    parent_id: String,
    port: u16,
    is_super: bool,
    order: usize,
    extension: String,
    index_types: Arc<serde_json::Value>,
    tx: Sender<EmbeddingJob>,
) -> Result<()> {
    let code_entity = CodeEntity {
        entity_type: owned.kind.clone(),
        start_byte: owned.start_byte,
        end_byte: owned.end_byte,
        order,
        text: owned.text.clone(),
    };
    if extension == "py" && code_entity.entity_type == "block" && !owned.children.is_empty() {
        let mut order = 1;
        for child in owned.children.into_iter() {
            process_entity(child, parent_id.clone(), port, false, order, extension.clone(), index_types.clone(), tx.clone()).await?;
            order += 1;
        }
    } else {
        // Handle special extension cases
        let mut index_type = extension.clone();
        if extension == "cc" || extension == "cxx" {
            index_type = "cpp".to_string();
        } else if extension == "h" {
            index_type = "c".to_string();
        } else if extension == "js" || extension == "jsx" {
            index_type = "js".to_string();
        }
        if let Some(types) = index_types.get(&index_type) {
            if let Some(types_array) = types.as_array() {
                let entity_type = &code_entity.entity_type;
                if types_array.iter().any(|v| v.as_str().map_or(false, |s| s == entity_type)) || 
                types_array.iter().any(|v| v.as_str().map_or(false, |s| s == "ALL")) {
                    let endpoint = if is_super {"createSuperEntity"} else {"createSubEntity"};
                    let url = format!("http://localhost:{}/{}", port, endpoint);
                    let id_name = if is_super {"file_id"} else {"entity_id"};
                    let payload = json!({
                        id_name: parent_id.clone(),
                        "entity_type": code_entity.entity_type,
                        "text": code_entity.text,
                        "start_byte": code_entity.start_byte,
                        "end_byte": code_entity.end_byte,
                        "order": code_entity.order,
                    });
                    let entity_response = post_request_async(&url, payload).await?;
                    let entity_id = entity_response
                        .get("entity")
                        .and_then(|v| v.get("id"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .ok_or_else(|| anyhow::anyhow!("Entity ID not found"))?;
                    if is_super {
                        let chunks = chunk_entity(&code_entity.text).unwrap();
                        TOTAL_CHUNKS.fetch_add(chunks.len(), Ordering::SeqCst);
                        let chunk_tasks: Vec<JoinHandle<()>> = chunks.into_iter().map(|chunk| {
                            let chunk_clone = chunk.clone();
                            let entity_id_clone = entity_id.clone();
                            let tx_clone = tx.clone();
                            tokio::spawn(async move {
                                let job = EmbeddingJob {chunk: chunk_clone, entity_id: entity_id_clone, port};
                                match tx_clone.try_send(job) {
                                    Ok(_) => {},
                                    Err(err) => {
                                        let job_back = err.into_inner();
                                        let tx_async = tx_clone.clone();
                                        tokio::spawn(async move {
                                            if let Err(e) = tx_async.send(job_back).await {
                                                eprintln!("Failed to send embedding job asynchronously: {}", e);
                                            }
                                        });
                                    }
                                }
                            })
                        }).collect();
                        join_all(chunk_tasks).await;
                    }
                    if !owned.children.is_empty() {
                        let order_counter = Arc::new(AtomicUsize::new(1));
                        let child_tasks: Vec<JoinHandle<Result<()>>> = owned.children.into_iter().map(|child| {
                            let entity_id_clone = entity_id.clone();
                            let extension_clone = extension.clone();
                            let index_types_clone = index_types.clone();
                            let tx_clone = tx.clone();
                            let order_counter_clone = order_counter.clone();
                            tokio::spawn(async move {
                                let current_order = order_counter_clone.fetch_add(1, Ordering::SeqCst);
                                process_entity(child, entity_id_clone, port, false, current_order, extension_clone, index_types_clone, tx_clone).await
                            })
                        }).collect();
                        for task in child_tasks {
                            task.await??;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn build_owned_nodes(node: Node, source: &str) -> Vec<OwnedNode> {
    let mut res = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let text = source[child.start_byte()..child.end_byte()].to_string();
        let owned = OwnedNode {
            kind: child.kind().to_string(),
            start_byte: child.start_byte(),
            end_byte: child.end_byte(),
            text,
            children: build_owned_nodes(child, source),
        };
        res.push(owned);
    }
    res
}