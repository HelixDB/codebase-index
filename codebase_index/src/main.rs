mod utils;

// External crates
use anyhow::Result;
use ignore::WalkBuilder;
use rayon::prelude::*;
use rayon::ThreadPoolBuilder;
use serde_json::json;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc
};
use std::time::Instant;
use tokio::runtime::Runtime;
use tree_sitter::Node;
use tree_sitter::Parser;
use chrono::{DateTime, Utc};

// Internal utility functions
use utils::{chunk_entity, embed_entity, get_language, post_request, set_embedding_rate_limit, CodeEntity};

/// Main entry point for the codebase ingestion tool
///
/// # Arguments (command line)
/// 1. path: Directory path to index (default: "sample")
/// 2. port: Port number for Helix server (default: 6969)
/// 3. concur_limit: Maximum concurrent operations (default: 36)
/// 4. rate_limit: Embedding API rate limit in requests per minute (default: 60)
fn main() -> Result<()> {
    // Parse command line arguments with defaults
    let args: Vec<String> = env::args().collect();

    // Get current directory
    let default_port = 6969;
    let max_concur = 48;

    // Get arguments
    let path: String = if args.len() > 1 {
        args[1].clone()
    } else {
        "sample".to_string()
    };
    let port: u16 = if args.len() > 2 {
        args[2].parse::<u16>().unwrap()
    } else {
        default_port
    };
    let concur_limit: usize = if args.len() > 3 {
        args[3].parse::<usize>().unwrap_or(max_concur)
    } else {
        max_concur
    };
    
    // Get embedding rate limit (requests per minute)
    let rate_limit: u64 = if args.len() > 4 {
        args[4].parse::<u64>().unwrap_or(1000)
    } else {
        1000
    };

    println!("\nConnecting to Helix instance at port {}", port);
    println!("Setting embedding rate limit to {} requests per minute", rate_limit);
    set_embedding_rate_limit(rate_limit);
    
    let start_time = Instant::now();
    
    // Initialize the global thread pool with the concurrency limit
    ThreadPoolBuilder::new()
        .num_threads(concur_limit)
        .build_global()
        .unwrap();
    
    // Create a global Tokio runtime for async operations
    let runtime = Arc::new(Runtime::new().unwrap());
    
    println!("Initialized thread pool with {} threads", concur_limit);

    // Start the ingestion process
    let root_id = ingestion(
        PathBuf::from(path).canonicalize()?,
        port,
        concur_limit,
        Arc::clone(&runtime),
    )?;

    // Wait for all background threads to complete
    println!("\nWaiting for background tasks to complete...");
    while ACTIVE_THREADS.load(Ordering::SeqCst) > 0 {
        // Poll the counter every 10ms
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    println!(
        "\nIngestion finished in {} seconds",
        start_time.elapsed().as_secs_f64()
    );

    println!("Root ID: {}", root_id);
    Ok(())
}

// Global thread counter to track active background threads
static ACTIVE_THREADS: AtomicUsize = AtomicUsize::new(0);

pub fn ingestion(
    root_path: PathBuf,
    port: u16,
    _concur_limit: usize,
    runtime: Arc<Runtime>,
) -> Result<String> {
    println!("Starting ingestion for directory: {}", root_path.display());

    // Create a root entry in the index
    let root_name = root_path.file_name().unwrap().to_str().unwrap();
    let url = format!("http://localhost:{}/createRoot", port);
    let payload = json!({ "name": root_name });

    // Send request to create root and get its ID
    let root_response = post_request(&url, payload, &runtime)?;
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

    // Start populating the index with directory contents
    populate(
        root_path,
        root_id.to_string(),
        port,
        true,
        index_types,
        Arc::clone(&runtime),
    )?;

    Ok(root_id.to_string())
}

/// Recursively populates the index with directory contents
fn populate(
    current_path: PathBuf,
    parent_id: String,
    port: u16,
    is_super: bool,
    index_types: Arc<serde_json::Value>,
    runtime: Arc<Runtime>,
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

    // Process entries in parallel using the thread pool
    entries.par_iter().for_each(|entry| {
        let path = entry.path();
        let path_buf = path.to_path_buf();
        let parent_id_clone = parent_id.clone();
        let index_types_clone = Arc::clone(&index_types);
        let runtime_clone = Arc::clone(&runtime);

        if path_buf.is_dir() {
            // Get folder information
            let folder_name = path_buf.file_name().unwrap().to_str().unwrap();
            let endpoint = if is_super {
                "createSuperFolder"
            } else {
                "createSubFolder"
            };
            let url = format!("http://localhost:{}/{}", port, endpoint);
            let payload = if is_super {
                json!({ "name": folder_name, "root_id": parent_id_clone })
            } else {
                json!({ "name": folder_name, "folder_id": parent_id_clone })
            };

            // Send request to create folder and get its ID
            println!("\nSubmitting {} folder for processing", folder_name);
            match post_request(&url, payload, &runtime_clone) {
                Ok(res) => {
                    if let Some(folder_id) = res
                        .get(if is_super { "folder" } else { "subfolder" })
                        .and_then(|v| v.get("id"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                    {
                        // Increment thread counter
                        ACTIVE_THREADS.fetch_add(1, Ordering::SeqCst);
                        
                        // Use rayon's spawn to process the folder in the thread pool
                        let path_buf_clone = path_buf.clone();
                        let folder_name_clone = folder_name.to_string();
                        let index_types_inner = Arc::clone(&index_types_clone);
                        let runtime_inner = Arc::clone(&runtime_clone);
                        
                        rayon::spawn(move || {
                            // Use a guard to ensure counter is decremented when thread exits
                            struct ThreadGuard;
                            impl Drop for ThreadGuard {
                                fn drop(&mut self) {
                                    ACTIVE_THREADS.fetch_sub(1, Ordering::SeqCst);
                                }
                            }
                            let _guard = ThreadGuard;

                            if let Err(e) = populate(
                                path_buf_clone,
                                folder_id,
                                port,
                                false,
                                index_types_inner,
                                runtime_inner,
                            ) {
                                eprintln!(
                                    "Error populating folder {}: {}",
                                    folder_name_clone, e
                                );
                            }
                        });
                    } else {
                        eprintln!(
                            "Failed to extract folder ID from response for: {}",
                            folder_name
                        );
                    }
                }
                Err(e) => {
                    eprintln!("Failed to create folder {}: {}", folder_name, e);
                }
            }
        // Path is a file
        } else if path_buf.is_file() {
            // Increment thread counter
            ACTIVE_THREADS.fetch_add(1, Ordering::SeqCst);
            
            // Process the file using the thread pool
            let path_buf_clone = path_buf.clone();
            let parent_id_clone2 = parent_id_clone.clone();
            let index_types_inner = Arc::clone(&index_types_clone);
            let runtime_inner = Arc::clone(&runtime_clone);
            
            rayon::spawn(move || {
                // Use a guard to ensure counter is decremented when thread exits
                struct ThreadGuard;
                impl Drop for ThreadGuard {
                    fn drop(&mut self) {
                        ACTIVE_THREADS.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                let _guard = ThreadGuard;

                process_file(
                    path_buf_clone,
                    parent_id_clone2,
                    is_super,
                    port,
                    index_types_inner,
                    runtime_inner,
                )
                .ok();
            });
        }
    });

    Ok(())
}

/// Processes a single file and extracts entities
fn process_file(
    file_path: PathBuf,
    parent_id: String,
    is_super: bool,
    port: u16,
    index_types: Arc<serde_json::Value>,
    runtime: Arc<Runtime>,
) -> Result<()> {
    // Read file contents
    let source_code = fs::read_to_string(&file_path)?;
    let file_name = file_path.file_name().unwrap().to_str().unwrap();
    let metadata = fs::metadata(&file_path)?;
    if let Ok(last_modified) = metadata.modified() {
        let date_modified = DateTime::<Utc>::from(last_modified).to_rfc3339();
        println!("{} last modified at {}", file_name, date_modified);
    } else {
        println!("{} last modified time not available", file_name);
    }
    let extension = file_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("txt");

    // Parse file with Tree Sitter
    if let Some(language) = get_language(&file_path) {
        // Parse file
        let mut parser = Parser::new();
        parser.set_language(&language)?;
        let tree = parser.parse(&source_code, None).unwrap();

        // Create file
        let file_type = if is_super { "super" } else { "sub" };
        let endpoint = if is_super {
            "createSuperFile"
        } else {
            "createFile"
        };
        let url = format!("http://localhost:{}/{}", port, endpoint);
        let payload = if is_super {
            json!({ "name": file_name, "extension": extension, "root_id": parent_id, "text": source_code })
        } else {
            json!({ "name": file_name, "extension": extension, "folder_id": parent_id, "text": source_code })
        };

        // Send request to create file
        println!("\nProcessing {} file: {}", file_type, file_name);
        let file_response = match post_request(&url, payload, &runtime) {
            Ok(response) => response,
            Err(e) => {
                eprintln!("Failed to create file {}: {}", file_name, e);
                eprintln!(
                    "This could indicate that the Helix server is not running or not responding."
                );
                eprintln!(
                    "Check that the server is running at http://localhost:{}",
                    port
                );
                return Err(anyhow::anyhow!("Failed to create file: {}", e));
            }
        };

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
        let mut binding = root_node.walk();
        
        // Collect children into a Vec to enable parallel iteration
        let children: Vec<Node> = root_node.children(&mut binding).collect();
        println!("Processing {} super entities from file {}", children.len(), file_name);
        
        // Use a shared counter for order to ensure consistent ordering
        let order_counter = Arc::new(AtomicUsize::new(1));
        
        // Process entities in parallel
        children.par_iter().for_each(|entity| {
            // Increment thread counter
            ACTIVE_THREADS.fetch_add(1, Ordering::SeqCst);
            
            // Use a guard to ensure counter is decremented when thread exits
            struct ThreadGuard;
            impl Drop for ThreadGuard {
                fn drop(&mut self) {
                    ACTIVE_THREADS.fetch_sub(1, Ordering::SeqCst);
                }
            }
            let _guard = ThreadGuard;
            
            // Get the current order value and increment it atomically
            let current_order = order_counter.fetch_add(1, Ordering::SeqCst);
            
            // Clone necessary variables for the closure
            let source_code_clone = source_code.to_string();
            let file_id_clone = file_id.to_string();
            let extension_clone = extension.to_string();
            let index_types_clone = Arc::clone(&index_types);
            let runtime_clone = Arc::clone(&runtime);
            
            // Get index_types for file extension
            if let Some(types) = index_types.get(&extension) {
                if let Some(types_array) = types.as_array() {
                    // Check if ALL is not in index_types
                    if types_array.iter().any(|v| v.as_str().map_or(false, |s| s != "ALL")) {
                        // Super entity type in index_types
                        if types_array.iter().any(|v| v.as_str().map_or(false, |s| s == entity.kind().to_string())){
                            // Has super content (need to create super entity and embed before processing current super entity)
                            let entity_start_byte = entity.start_byte();
                            let entity_end_byte = entity.end_byte();
                            let entity_content = &source_code_clone[entity_start_byte..entity_end_byte].to_string();
                            
                            let endpoint = "createSuperEntity";
                            let url = format!("http://localhost:{}/{}", port, endpoint);
                            let payload = json!({
                                "file_id": file_id_clone.clone(),
                                "entity_type": entity.kind().to_string(),
                                "text": entity_content,
                                "start_byte": entity_start_byte,
                                "end_byte": entity_end_byte,
                                "order": current_order,
                            });
                            
                            // Send request to create entity
                            if let Ok(entity_response) = post_request(&url, payload, &runtime_clone) {
                                if let Some(entity_id) = entity_response
                                    .get("entity")
                                    .and_then(|v| v.get("id"))
                                    .and_then(|v| v.as_str())
                                {
                                    // Chunk entity text
                                    if let Ok(chunks) = chunk_entity(&entity_content) {
                                        // Process chunks in parallel
                                        chunks.par_iter().for_each(|chunk| {
                                            let chunk_clone = chunk.clone();
                                            let entity_id_clone = entity_id.to_string();
                                            
                                            // Increment thread counter for embedding
                                            ACTIVE_THREADS.fetch_add(1, Ordering::SeqCst);
                                            
                                            // Use a guard to ensure counter is decremented when thread exits
                                            struct EmbedThreadGuard;
                                            impl Drop for EmbedThreadGuard {
                                                fn drop(&mut self) {
                                                    ACTIVE_THREADS.fetch_sub(1, Ordering::SeqCst);
                                                }
                                            }
                                            let _embed_guard = EmbedThreadGuard;
                                            
                                            // Generate embedding
                                            if let Ok(embedding) = embed_entity(chunk_clone, &runtime_clone) {
                                                let embed_endpoint = "embedSuperEntity";
                                                let payload = json!({
                                                    "entity_id": entity_id_clone,
                                                    "vector": embedding,
                                                });
                                                let embed_url = format!("http://localhost:{}/{}", port, embed_endpoint);
                                                
                                                // Post embedding
                                                if let Err(e) = post_request(&embed_url, payload, &runtime_clone) {
                                                    eprintln!("Error posting embedding: {}", e);
                                                }
                                            }
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
            
            // Process entity and its children
            if let Err(e) = process_entity(
                *entity,
                &source_code_clone,
                file_id_clone,
                port,
                true,
                current_order,
                extension_clone,
                index_types_clone,
                runtime_clone,
            ) {
                eprintln!("Error processing entity: {}", e);
            }
        });
    // File is not supported by Tree Sitter
    } else {
        // Create file without entities
        let endpoint = if is_super {
            "createSuperFile"
        } else {
            "createFile"
        };
        let url = format!("http://localhost:{}/{}", port, endpoint);
        let payload = if is_super {
            json!({ "name": file_name, "extension": extension, "root_id": parent_id, "text": source_code })
        } else {
            json!({ "name": file_name, "extension": extension, "folder_id": parent_id, "text": source_code })
        };

        // Send request to create file
        println!("\nProcessing unsupported file: {}", file_name);
        post_request(&url, payload, &runtime)?;
    }
    Ok(())
}

/// Processes an entity and its children recursively
fn process_entity(
    entity: Node,
    source_code: &str,
    parent_id: String,
    port: u16,
    is_super: bool,
    order: usize,
    extension: String,
    index_types: Arc<serde_json::Value>,
    runtime: Arc<Runtime>,
) -> Result<()> {
    // Create entity
    let code_entity = CodeEntity {
        entity_type: entity.kind().to_string(),
        start_byte: entity.start_byte(),
        end_byte: entity.end_byte(),
        order,
        text: source_code[entity.start_byte()..entity.end_byte()].to_string(),
    };
    let mut binding = entity.walk();
    let children = entity.children(&mut binding);
    let len = children.len();

    // Special case for Python
    if extension == "py" && code_entity.entity_type == "block" && len > 0 {
        // Recursively process children of entity
        if len > 0 {
            let mut order = 1;
            for child in children.into_iter() {
                process_entity(
                    child,
                    &source_code,
                    parent_id.to_string(),
                    port,
                    false,
                    order,
                    extension.clone(),
                    Arc::clone(&index_types),
                    Arc::clone(&runtime),
                )?;
                order += 1;
            }
        }
    }
    // General case
    else {
        if let Some(types) = index_types.get(&extension) {
            if let Some(types_array) = types.as_array() {
                let entity_type = &code_entity.entity_type;
                if types_array.iter().any(|v| v.as_str().map_or(false, |s| s == entity_type)) || types_array.iter().any(|v| v.as_str().map_or(false, |s| s == "ALL")) {
                    let endpoint = if is_super {
                        "createSuperEntity"
                    } else {
                        "createSubEntity"
                    };
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

                    // Send request to create entity
                    let entity_response = post_request(&url, payload, &runtime)?;
                    // Convert to owned String to avoid lifetime issues with threads
                    let entity_id = entity_response
                        .get("entity")
                        .and_then(|v| v.get("id"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .ok_or_else(|| anyhow::anyhow!("Entity ID not found"))?;

                    // Embed entity if super entity
                    if is_super {
                        // Chunk entity text
                        let chunks = chunk_entity(&code_entity.text).unwrap();
                        // println!("chunks length: {}", chunks.len());
                        // println!("Chunking entity: {}", code_entity.text);

                        // Create a counter to track when all embeddings are complete
                        let embedding_counter = Arc::new(AtomicUsize::new(chunks.len()));
                        let embedding_done = Arc::new(AtomicBool::new(false));

                        // Process chunks in parallel using rayon
                        chunks.par_iter().for_each(|chunk| {
                            let chunk_clone = chunk.clone();
                            let entity_id_clone = entity_id.clone();
                            let counter = Arc::clone(&embedding_counter);
                            let done_flag: Arc<AtomicBool> = Arc::clone(&embedding_done);

                            // Increment thread counter
                            ACTIVE_THREADS.fetch_add(1, Ordering::SeqCst);

                            // Process embedding in the thread pool
                            let embedding = embed_entity(chunk_clone, &runtime).unwrap();
                            let embed_endpoint = "embedSuperEntity";
                            let payload = json!({
                                "entity_id": entity_id_clone,
                                "vector": embedding,
                            });
                            let embed_url = format!("http://localhost:{}/{}", port, embed_endpoint);

                            // Send the request
                            if let Err(e) = post_request(&embed_url, payload, &runtime) {
                                eprintln!("Error posting embedding: {}", e);
                            }

                            // Decrement counters
                            ACTIVE_THREADS.fetch_sub(1, Ordering::SeqCst);

                            // Update the completion counter atomically
                            let remaining = counter.fetch_sub(1, Ordering::SeqCst);
                            if remaining == 1 { // This was the last chunk
                                done_flag.store(true, Ordering::SeqCst);
                            }
                        });
                    }

                    // Recursively process children of entity in parallel
                    if len > 0 {
                        // Use a shared counter for order to ensure consistent ordering
                        let order_counter = Arc::new(AtomicUsize::new(1));
                        
                        // Convert children into a Vec to enable parallel iteration
                        let children_vec: Vec<_> = children.collect();
                        
                        // Process children in parallel
                        children_vec.into_par_iter().for_each(|child| {
                            // Increment thread counter
                            ACTIVE_THREADS.fetch_add(1, Ordering::SeqCst);
                            
                            // Use a guard to ensure counter is decremented when thread exits
                            struct ThreadGuard;
                            impl Drop for ThreadGuard {
                                fn drop(&mut self) {
                                    ACTIVE_THREADS.fetch_sub(1, Ordering::SeqCst);
                                }
                            }
                            let _guard = ThreadGuard;
                            
                            // Get the current order value and increment it atomically
                            let current_order = order_counter.fetch_add(1, Ordering::SeqCst);
                            
                            // Clone necessary variables for the closure
                            let source_code_clone = source_code.to_string();
                            let entity_id_clone = entity_id.to_string();
                            let extension_clone = extension.clone();
                            let index_types_clone = Arc::clone(&index_types);
                            let runtime_clone = Arc::clone(&runtime);
                            
                            // Process child entity
                            if let Err(e) = process_entity(
                                child,
                                &source_code_clone,
                                entity_id_clone,
                                port,
                                false,
                                current_order,
                                extension_clone,
                                index_types_clone,
                                runtime_clone,
                            ) {
                                eprintln!("Error processing child entity: {}", e);
                            }
                        })
                    }
                }
            }
        }
    }

    Ok(())
}
