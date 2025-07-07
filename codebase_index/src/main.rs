mod utils;

// External crates
use anyhow::Result;
use ignore::WalkBuilder;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::fs;
use parking_lot::Mutex;
use std::thread;
use tree_sitter::Parser;
use std::time::Instant;
use std::env;

// Internal utility functions
use utils::{
    chunk_entity,
    embed_entity,
    extract_entities_recursive,
    get_language,
    post_request,
    CodeEntity
};

/// Main entry point for the codebase ingestion tool
/// 
/// # Arguments (command line)
/// 1. path: Directory path to index (default: "sample")
/// 2. port: Port number for Helix server (default: 6969)
/// 3. max_depth: Maximum depth for entity processing (default: 2)
/// 4. concur_limit: Maximum concurrent operations (default: 10)
fn main() -> Result<()> {
    // Parse command line arguments with defaults
    let args: Vec<String> = env::args().collect();

    // Get arguments
    let path:String = if args.len() > 1 {args[1].clone()} else {"sample".to_string()};
    let port:u16 = if args.len() > 2 {args[2].parse::<u16>().unwrap()} else {6969};
    let max_depth:usize = if args.len() > 3 {args[3].parse::<usize>().unwrap()} else {2};
    let concur_limit:usize = if args.len() > 4 {args[4].parse::<usize>().unwrap()} else {10};
    
    println!("\nConnecting to Helix instance at port {}", port);
    let start_time = Instant::now();
    
    // Start the ingestion process
    ingestion(PathBuf::from(path).canonicalize()?, port, max_depth, concur_limit)?;
    
    // Add a delay to allow background threads to complete
    println!("\nWaiting for background tasks to complete...");
    std::thread::sleep(std::time::Duration::from_secs(2));
    
    println!("\nIngestion finished in {} seconds", start_time.elapsed().as_secs_f64());
    Ok(())
}

pub fn ingestion(root_path: PathBuf, port: u16, max_depth: usize, _concur_limit: usize) -> Result<()> {
    println!("Starting ingestion for directory: {}", root_path.display());

    // Create a root entry in the index
    let root_name = root_path.file_name().unwrap().to_str().unwrap();
    let url = format!("http://localhost:{}/createRoot", port);
    let payload = json!({ "name": root_name });
    
    // Send request to create root and get its ID
    let root_response = post_request(&url, payload)?;
    let root_id = root_response
        .get("root")
        .and_then(|f| f.get(0))
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Root ID not found"))?;

    println!("\nRoot created");

    // Create a mutex to limit concurrency
    let semaphore = Arc::new(Mutex::new(()));

    // Start populating the index with directory contents
    populate(root_path, root_id.to_string(), port, semaphore, true, max_depth)?;

    println!("\nIngestion finished");
    Ok(())
}

/// Recursively populates the index with directory contents
fn populate(
    current_path: PathBuf,
    parent_id: String,
    port: u16,
    semaphore: Arc<Mutex<()>>,
    is_super: bool,
    max_depth: usize,
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

        let handle = thread::spawn(move || {
            // Acquire lock to limit concurrency
            let _guard = semaphore_clone.lock();

            if path_buf.is_dir() {
                // Get folder information
                let folder_name = path_buf.file_name().unwrap().to_str().unwrap();
                let endpoint = if is_super { "createSuperFolder" } else { "createSubFolder" };
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
                        if let Some(folder_id) = res.get("folder").and_then(|f| f.get(0)).and_then(|v| v.get("id")).and_then(|v| v.as_str()) {
                            println!("Successfully created folder: {} with ID: {}", folder_name, folder_id);
                            
                            // Recursively populate the folder
                            let inner_semaphore = Arc::clone(&semaphore_clone);
                            let path_buf_clone = path_buf.clone();
                            let folder_id_string = folder_id.to_string();
                            let folder_name_clone = folder_name.to_string();
                            
                            // Release the lock before spawning the new thread
                            drop(_guard);
                            
                            // Spawn a new thread for the recursive call
                            thread::spawn(move || {
                                println!("Starting recursive processing of folder: {}", folder_name_clone);
                                if let Err(e) = populate(path_buf_clone, folder_id_string, port, inner_semaphore, false, max_depth) {
                                    eprintln!("Error populating folder {}: {}", folder_name_clone, e);
                                }
                            });
                        } else {
                            eprintln!("Failed to extract folder ID from response for: {}", folder_name);
                        }
                    },
                    Err(e) => {
                        eprintln!("Failed to create folder {}: {}", folder_name, e);
                    }
                }
            // Path is a file
            } else if path_buf.is_file() {
                // Process file - clone path_buf to avoid borrowing issues
                let path_buf_clone = path_buf.clone();
                let parent_id_clone2 = parent_id_clone.clone();
                
                // Release the lock before processing the file
                drop(_guard); // Explicitly drop the guard to release the lock
                
                // Process the file in a separate thread
                thread::spawn(move || {
                    println!("Processing file: {}", path_buf_clone.display());
                    process_file(path_buf_clone, parent_id_clone2, is_super, port, max_depth).ok();
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
    max_depth: usize,
) -> Result<()> {
    // Read file contents
    let source_code = fs::read_to_string(&file_path)?;
    let file_name = file_path.file_name().unwrap().to_str().unwrap();
    let extension = file_path.extension().and_then(|s| s.to_str()).unwrap_or("txt");

    // Parse file with Tree Sitter
    if let Some(language) = get_language(&file_path) {
        // Parse file
        let mut parser = Parser::new();
        parser.set_language(language)?;
        let tree = parser.parse(&source_code, None).unwrap();
        let tree_dict = extract_entities_recursive(tree.root_node(), &source_code);

        // Create file
        let file_type = if is_super { "super" } else { "sub" };
        let endpoint = if is_super { "createSuperFile" } else { "createFile" };
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
                eprintln!("This could indicate that the Helix server is not running or not responding.");
                eprintln!("Check that the server is running at http://localhost:{}", port);
                return Err(anyhow::anyhow!("Failed to create file: {}", e));
            }
        };
        
        let file_id = file_response.get("file")
            .and_then(|f| f.get(0))
            .and_then(|v| v.get("id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                eprintln!("Failed to extract file ID from response for: {}", file_name);
                anyhow::anyhow!("File ID not found in response")
            })?;

        // Process entities
        println!("\nProcessing {} super entities from file {}", tree_dict.len(), file_name);
        for entity in tree_dict.into_iter() {
            process_entity(entity, file_id.to_string(), port, true, 0, max_depth)?;
        }
    // File is not supported by Tree Sitter
    } else {
        // Create file without entities
        let endpoint = if is_super { "createSuperFile" } else { "createFile" };
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
    entity: CodeEntity,
    parent_id: String,
    port: u16,
    is_super: bool,
    depth: usize,
    max_depth: usize,
) -> Result<()> {
    // Create entity
    let endpoint = if is_super { "createSuperEntity" } else { "createSubEntity" };
    let url = format!("http://localhost:{}/{}", port, endpoint);
    let payload = if is_super {
            json!({
                "file_id": parent_id,
                "entity_type": entity.entity_type,
                "text": entity.text,
                "start_byte": entity.start_byte,
                "end_byte": entity.end_byte,
                "order": entity.order,
            })
        } else {
            json!({
                "entity_id": parent_id,
                "entity_type": entity.entity_type,
                "text": entity.text,
                "start_byte": entity.start_byte,
                "end_byte": entity.end_byte,
                "order": entity.order,
            })
        };

    // Send request to create entity
    let entity_response = post_request(&url, payload)?;
    let entity_id = entity_response.get("entity")
        .and_then(|f| f.get(0))
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Entity ID not found"))?;

    // Embed entity if super entity
    if is_super {
        // Chunk entity text
        let chunks = chunk_entity(&entity.text);

        // Embed each chunk
        for chunk in chunks {
            let embedding = embed_entity(chunk);
            let embed_endpoint = "embedSuperEntity";
            let payload = json!({
                "entity_id": entity_id,
                "vector": embedding,
            });
            let embed_url = format!("http://localhost:{}/{}", port, embed_endpoint);
            post_request(&embed_url, payload)?;
        }
    }

    // Recursively process children of entity not at max depth
    if depth < max_depth && !entity.children.is_empty() {
        for child in entity.children.into_iter() {
            process_entity(child, entity_id.to_string(), port, false, depth + 1, max_depth)?;
        }
    }
    
    Ok(())
}
