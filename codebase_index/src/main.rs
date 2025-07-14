mod utils;

// External crates
use anyhow::Result;
use ignore::WalkBuilder;
use parking_lot::Mutex;
use serde_json::json;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::thread;
use std::time::Instant;
use tree_sitter::Node;
use tree_sitter::Parser;
use chrono::{DateTime, Utc};

// Internal utility functions
use utils::{chunk_entity, embed_entity, get_language, post_request, CodeEntity};

/// Main entry point for the codebase ingestion tool
///
/// # Arguments (command line)
/// 1. path: Directory path to index (default: "sample")
/// 2. port: Port number for Helix server (default: 6969)
/// 3. concur_limit: Maximum concurrent operations (default: 10)
fn main() -> Result<()> {
    // Parse command line arguments with defaults
    let args: Vec<String> = env::args().collect();

    // Get current directory
    let default_port = 6969;
    let max_concur = 20;

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
        args[3].parse::<usize>().unwrap()
    } else {
        max_concur
    };

    println!("\nConnecting to Helix instance at port {}", port);
    let start_time = Instant::now();

    // Start the ingestion process
    ingestion(
        PathBuf::from(path).canonicalize()?,
        port,
        concur_limit,
    )?;

    // Wait for all background threads to complete
    println!("\nWaiting for background tasks to complete...");
    while ACTIVE_THREADS.load(Ordering::SeqCst) > 0 {
        // Poll the counter every 100ms
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    println!(
        "\nIngestion finished in {} seconds",
        start_time.elapsed().as_secs_f64()
    );
    Ok(())
}

// Global thread counter to track active background threads
static ACTIVE_THREADS: AtomicUsize = AtomicUsize::new(0);

pub fn ingestion(
    root_path: PathBuf,
    port: u16,
    _concur_limit: usize,
) -> Result<()> {
    println!("Starting ingestion for directory: {}", root_path.display());

    // Create a root entry in the index
    let root_name = root_path.file_name().unwrap().to_str().unwrap();
    let url = format!("http://localhost:{}/createRoot", port);
    let payload = json!({ "name": root_name });

    // Send request to create root and get its ID
    let root_response = post_request(&url, payload)?;
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

    // Create a mutex to limit concurrency
    let semaphore = Arc::new(Mutex::new(_concur_limit));

    // Start populating the index with directory contents
    populate(
        root_path,
        root_id.to_string(),
        port,
        semaphore,
        true,
        index_types,
    )?;

    Ok(())
}

/// Recursively populates the index with directory contents
fn populate(
    current_path: PathBuf,
    parent_id: String,
    port: u16,
    semaphore: Arc<Mutex<usize>>,
    is_super: bool,
    index_types: Arc<serde_json::Value>,
) -> Result<()> {
    // Initialize tasks and walker builder
    let mut handles = Vec::new();
    let mut walker_builder = WalkBuilder::new(&current_path);
    walker_builder.max_depth(Some(1));

    // Add default ignore patterns
    for pattern in &[".git/"] {
        walker_builder.add_ignore(pattern);
    }

    // Walk the directory
    for result in walker_builder.build() {
        // Handle directory traversal errors
        let entry = match result {
            Ok(entry) => entry,
            Err(e) => {
                eprintln!("Error walking directory: {}", e);
                continue;
            }
        };

        // Skip the current directory
        let path = entry.path();
        if path == current_path {
            continue;
        }

        // Clone semaphore and parent ID for task
        let semaphore_clone = Arc::clone(&semaphore);
        let parent_id_clone = parent_id.clone();
        let path_buf = path.to_path_buf();
        let index_types_clone = Arc::clone(&index_types);

        let handle = thread::spawn(move || {
            // Acquire lock to limit concurrency
            let _guard = semaphore_clone.lock();

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
                match post_request(&url, payload) {
                    Ok(res) => {
                        if let Some(folder_id) = res
                            .get("folder")
                            .and_then(|v| v.get("id"))
                            .and_then(|v| v.as_str())
                        {
                            // Recursively populate the folder
                            let inner_semaphore = Arc::clone(&semaphore_clone);
                            let path_buf_clone = path_buf.clone();
                            let folder_id_string = folder_id.to_string();
                            let folder_name_clone = folder_name.to_string();
                            let index_types_inner = Arc::clone(&index_types_clone);

                            // Release the lock before spawning the new thread
                            drop(_guard);

                            // Increment thread counter before spawning
                            ACTIVE_THREADS.fetch_add(1, Ordering::SeqCst);

                            // Spawn a new thread for the recursive call
                            thread::spawn(move || {
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
                                    folder_id_string,
                                    port,
                                    inner_semaphore,
                                    false,
                                    index_types_inner,
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
                // Process file - clone path_buf to avoid borrowing issues
                let path_buf_clone = path_buf.clone();
                let parent_id_clone2 = parent_id_clone.clone();
                let index_types_inner = Arc::clone(&index_types_clone);

                // Release the lock before processing the file
                drop(_guard); // Explicitly drop the guard to release the lock

                // Increment thread counter before spawning
                ACTIVE_THREADS.fetch_add(1, Ordering::SeqCst);

                // Process the file in a separate thread
                thread::spawn(move || {
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
                    )
                    .ok();
                });
            }
            Ok::<(), anyhow::Error>(())
        });
        handles.push(handle);
    }

    // Wait for all tasks to complete
    for handle in handles {
        let _ = handle.join().expect("Thread panicked");
    }

    Ok(())
}

/// Processes a single file and extracts entities
fn process_file(
    file_path: PathBuf,
    parent_id: String,
    is_super: bool,
    port: u16,
    index_types: Arc<serde_json::Value>,
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
        let file_response = match post_request(&url, payload) {
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
        let children = root_node.children(&mut binding);
        println!(
            "\nProcessing {} super entities from file {}",
            children.len(),
            file_name
        );
        let mut order = 1;
        let mut super_start_byte: Option<usize> = None;
        let mut super_end_byte: Option<usize> = None;
        for entity in children.into_iter() {
            // Get index_types for file extension
            if let Some(types) = index_types.get(&extension) {
                if let Some(types_array) = types.as_array() {
                    // Check if ALL is not in index_types
                    if types_array.iter().any(|v| v.as_str().map_or(false, |s| s != "ALL")) {
                        // Super entity type in index_types
                        if types_array.iter().any(|v| v.as_str().map_or(false, |s| s == entity.kind().to_string())){
                            // Has super content (need to create super entity and embed before processing current super entity)
                            if super_start_byte.is_some() {
                                // Embed super content
                                let super_content = &source_code[super_start_byte.unwrap()..super_end_byte.unwrap()].to_string();
                                // println!("Embedding super content: \n{}", super_content);

                                let endpoint = "createSuperEntity";
                                let url = format!("http://localhost:{}/{}", port, endpoint);
                                let payload = json!({
                                    "file_id": file_id.to_string(),
                                    "entity_type": "super_code",
                                    "text": super_content,
                                    "start_byte": super_start_byte.unwrap(),
                                    "end_byte": super_end_byte.unwrap(),
                                    "order": order,
                                });

                                order += 1;
                                
                                let entity_response = post_request(&url, payload)?;
                                let entity_id = entity_response
                                    .get("entity")
                                    .and_then(|v| v.get("id"))
                                    .and_then(|v| v.as_str())
                                    .ok_or_else(|| anyhow::anyhow!("Entity ID not found"))?;

                                // Chunk entity text
                                let chunks = chunk_entity(&super_content).unwrap();
                                // println!("chunks length: {}", chunks.len());

                                // Embed each chunk
                                for chunk in chunks {
                                    let embedding = embed_entity(chunk).unwrap();
                                    let embed_endpoint = "embedSuperEntity";
                                    let payload = json!({
                                        "entity_id": entity_id,
                                        "vector": embedding,
                                    });
                                    let embed_url = format!("http://localhost:{}/{}", port, embed_endpoint);
                                    post_request(&embed_url, payload)?;
                                }

                                super_start_byte = None;
                                super_end_byte = None;
                            }
                        // Super entity type not in index_types
                        } else {
                            // Update super_start_byte and super_end_byte
                            if super_start_byte.is_none(){
                                super_start_byte = Some(entity.start_byte());
                            }
                            super_end_byte = Some(entity.end_byte());
                            // println!("Added Content: \n{}", &source_code[entity.start_byte()..entity.end_byte()]);
                        }
                    }
                }
            }
            process_entity(
                entity,
                &source_code,
                file_id.to_string(),
                port,
                true,
                order,
                extension.to_string(),
                Arc::clone(&index_types),
            )?;
            order += 1;
        }
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
        post_request(&url, payload)?;
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
                    let payload = if is_super {
                        json!({
                            "file_id": parent_id,
                            "entity_type": code_entity.entity_type,
                            "text": code_entity.text,
                            "start_byte": code_entity.start_byte,
                            "end_byte": code_entity.end_byte,
                            "order": code_entity.order,
                        })
                    } else {
                        json!({
                            "entity_id": parent_id,
                            "entity_type": code_entity.entity_type,
                            "text": code_entity.text,
                            "start_byte": code_entity.start_byte,
                            "end_byte": code_entity.end_byte,
                            "order": code_entity.order,
                        })
                    };

                    // Send request to create entity
                    let entity_response = post_request(&url, payload)?;
                    let entity_id = entity_response
                        .get("entity")
                        .and_then(|v| v.get("id"))
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow::anyhow!("Entity ID not found"))?;

                    // Embed entity if super entity
                    if is_super {
                        // Chunk entity text
                        let chunks = chunk_entity(&code_entity.text).unwrap();
                        // println!("chunks length: {}", chunks.len());
                        // println!("Chunking entity: {}", code_entity.text);

                        // Embed each chunk
                        for chunk in chunks {
                            let embedding = embed_entity(chunk).unwrap();
                            let embed_endpoint = "embedSuperEntity";
                            let payload = json!({
                                "entity_id": entity_id,
                                "vector": embedding,
                            });
                            let embed_url = format!("http://localhost:{}/{}", port, embed_endpoint);
                            post_request(&embed_url, payload)?;
                        }
                    }

                    // Recursively process children of entity
                    if len > 0 {
                        let mut order = 1;
                        for child in children.into_iter() {
                            process_entity(
                                child,
                                &source_code,
                                entity_id.to_string(),
                                port,
                                false,
                                order,
                                extension.clone(),
                                Arc::clone(&index_types),
                            )?;
                            order += 1;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
