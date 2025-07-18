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
    atomic::{AtomicUsize, Ordering},
    Arc
};
use std::time::Instant;
use tokio::runtime::Runtime;
use tree_sitter::Node;
use tree_sitter::Parser;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

// Internal utility functions
use utils::{chunk_entity, embed_entity_async, post_request_async, EmbeddingJob, get_language, post_request, CodeEntity};

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

    let default_port = 6969;
    let max_concur = 1000;
    let update_interval = 10;

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

    println!("\nConnecting to Helix instance at port {}", port);

    dotenv::dotenv().ok();
    
    let start_time = Instant::now();
    
    // Initialize the global thread pool with the concurrency limit
    ThreadPoolBuilder::new()
        .num_threads(concur_limit)
        .build_global()
        .unwrap();
    
    // Create a global Tokio runtime for async operations
    let runtime = Arc::new(Runtime::new().unwrap());

    // Create the Tokio mpsc channel for embedding jobs
    let (tx, mut rx) = tokio::sync::mpsc::channel::<EmbeddingJob>(concur_limit);

    // Spawn the async background task for embedding jobs
    runtime.spawn(async move {
        while let Some(job) = rx.recv().await {
            let EmbeddingJob { chunk, entity_id, port } = job;
            match embed_entity_async(chunk).await {
                Ok(embedding) => {
                    let url = format!("http://localhost:{}/embedSuperEntity", port);
                    let payload = json!({
                        "entity_id": entity_id,
                        "vector": embedding,
                    });
                    if let Err(e) = post_request_async(&url, payload).await {
                        eprintln!("Failed to post embedding: {}", e);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to embed chunk: {}", e);
                }
            }
        }
    });

    println!("Initialized thread pool with {} threads", concur_limit);

    // Start the ingestion process
    let root_id = ingestion(
        PathBuf::from(path.clone()).canonicalize()?,
        port,
        concur_limit,
        Arc::clone(&runtime),
        tx.clone(), // Pass the sender into ingestion
    )?;

    // Wait for all background threads to complete
    println!("\nWaiting for background tasks to complete...");
    while ACTIVE_THREADS.load(Ordering::SeqCst) > 0 {
        // Poll the counter every 10ms
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    // Get the total number of chunks processed
    let total_chunks = TOTAL_CHUNKS.load(Ordering::SeqCst);

    println!(
        "\nIngestion finished in {} seconds",
        start_time.elapsed().as_secs_f64()
    );

    println!("Root ID: {}", root_id);
    println!("Total chunks processed: {}", total_chunks);

    TOTAL_CHUNKS.store(0, Ordering::SeqCst);
    ACTIVE_THREADS.store(0, Ordering::SeqCst);

    loop {
        println!("\nUpdating index...");
        update(PathBuf::from(path.clone()).canonicalize()?, root_id.clone(), port, concur_limit, runtime.clone(), tx.clone(), 5)?;
        while ACTIVE_THREADS.load(Ordering::SeqCst) > 0 {
            // Poll the counter every 10ms
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        println!("\nTotal chunks processed: {}", TOTAL_CHUNKS.load(Ordering::SeqCst));

        TOTAL_CHUNKS.store(0, Ordering::SeqCst);
        ACTIVE_THREADS.store(0, Ordering::SeqCst);

        std::thread::sleep(std::time::Duration::from_secs(update_interval));
    }

}

// Global thread counter to track active background threads
static ACTIVE_THREADS: AtomicUsize = AtomicUsize::new(0);

// Global counter to track total number of chunks processed
static TOTAL_CHUNKS: AtomicUsize = AtomicUsize::new(0);

pub fn update(
    root_path: PathBuf,
    root_id: String,
    port: u16,
    _concur_limit: usize,
    runtime: Arc<Runtime>,
    tx: tokio::sync::mpsc::Sender<utils::EmbeddingJob>,
    update_interval: u64,
) -> Result<()> {    
    // Load index types
    let index_types = fs::read_to_string("index-types.json")?;
    let index_types: serde_json::Value = serde_json::from_str(&index_types)?;
    let index_types = Arc::new(index_types);

    // Check if root exists
    let url = format!("http://localhost:{}/{}", port, "getRootById");
    let payload = json!({ "root_id": root_id });
    let root_res = post_request(&url, payload, &runtime)?;
    let root = root_res
        .get("root")
        .and_then(|v| v.get("name"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Root not found"))?;

    let root_name = root_path.file_name().unwrap().to_str().unwrap();

    if root != root_name {
        return Err(anyhow::anyhow!("Root name does not match"));
    }

    // Get root folders
    let url = format!("http://localhost:{}/{}", port, "getRootFolders");
    let payload = json!({ "root_id": root_id });
    let root_folder_res = post_request(&url, payload, &runtime)?;
    let root_folders = root_folder_res
        .get("folders")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("Root ID not found"))?;

    let mut root_folder_name_ids = HashMap::new();

    for folder in root_folders {
        let folder_id = folder.get("id").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Folder ID not found"))?;
        let folder_name = folder.get("name").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Folder name not found"))?;
        root_folder_name_ids.insert(folder_name.to_string(), folder_id.to_string());
    }

    // println!("Root folder IDs: {:#?}", root_folder_name_ids);

    // Get root files
    let url = format!("http://localhost:{}/{}", port, "getRootFiles");
    let payload = json!({ "root_id": root_id });
    let root_file_res = post_request(&url, payload, &runtime)?;
    let root_files = root_file_res
        .get("files")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("Root ID not found"))?;

    let mut root_file_name_ids = HashMap::new();

    for file in root_files {
        let file_id = file.get("id").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("File ID not found"))?;
        let file_name = file.get("name").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("File name not found"))?;
        root_file_name_ids.insert(file_name.to_string(), file_id.to_string());
    }

    // println!("Root file IDs: {:#?}", root_file_name_ids);

    let mut walker_builder = WalkBuilder::new(&root_path);
    walker_builder.max_depth(Some(1));

    // Add default ignore patterns
    for pattern in &[".git/"] {
        walker_builder.add_ignore(pattern);
    }

    // Collect entries to process
    let entries: Vec<_> = walker_builder.build()
        .filter_map(|result| result.ok())
        .filter(|entry| entry.path() != root_path)
        .collect();

    entries.par_iter().for_each(|entry| {
        let path = entry.path();
        let path_buf = path.to_path_buf();
        let index_types_clone = Arc::clone(&index_types);
        let runtime_clone = Arc::clone(&runtime);
        let root_folder_name_ids_clone = root_folder_name_ids.clone();
        let tx_clone = tx.clone();

        // Folder
        if path_buf.is_dir(){
            let folder_name = path_buf.file_name().unwrap().to_str().unwrap();
            if root_folder_name_ids_clone.contains_key(folder_name){
                println!("Folder {} already exists", folder_name);
                let folder_id = root_folder_name_ids_clone.get(folder_name).unwrap().to_string();
                let _ = update_folder(path_buf.clone(), folder_id.clone(), port, index_types_clone, runtime_clone, tx_clone, update_interval);
            } else {
                println!("Folder {} does not exist", folder_name);
                let _ = populate(path_buf.clone(), root_id.clone(), port, true, index_types_clone, runtime_clone, tx_clone);
            }

        // File
        } else if path_buf.is_file() {
            // Increment thread counter
            ACTIVE_THREADS.fetch_add(1, Ordering::SeqCst);

            // Process the file using the thread pool
            let path_buf_clone = path_buf.clone();
            let root_id_clone = root_id.clone();
            let index_types_inner = Arc::clone(&index_types_clone);
            let runtime_inner = Arc::clone(&runtime_clone);
            let root_file_name_ids_clone = root_file_name_ids.clone();
            
            rayon::spawn(move || {
                // Use a guard to ensure counter is decremented when thread exits
                struct ThreadGuard;
                impl Drop for ThreadGuard {
                    fn drop(&mut self) {
                        ACTIVE_THREADS.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                let _guard = ThreadGuard;

                let file_name = path_buf_clone.file_name().unwrap().to_str().unwrap();
                
                if root_file_name_ids_clone.contains_key(file_name){
                    println!("File {} already exists", file_name);
                    let file_id = root_file_name_ids_clone.get(file_name).unwrap().to_string();
                    let metadata = fs::metadata(&path_buf_clone).expect("Failed to get metadata");
                    if let Ok(last_modified) = metadata.modified() {
                        let date_modified = DateTime::<Utc>::from(last_modified);

                        let url = format!("http://localhost:{}/{}", port, "getFile");
                        let payload = json!({ "file_id": file_id });
                        let file_res = post_request(&url, payload, &runtime_inner).expect("File not found");
                        let file = file_res.get("file").expect("File not found");
                        
                        let extracted_at = file.get("extracted_at").and_then(|v| v.as_str()).expect("File ID not found");
                        let date_extracted = DateTime::parse_from_rfc3339(extracted_at)
                            .expect("Failed to parse date")
                            .with_timezone(&Utc);

                        let diff_sec = date_modified.signed_duration_since(date_extracted).num_seconds();
                        if diff_sec > update_interval.try_into().unwrap() {
                            println!("File {} is out of date", file_name);
                            let _ = update_file(
                                path_buf_clone,
                                file_id,
                                port,
                                index_types_inner,
                                runtime_inner,
                                tx_clone.clone(),
                            );
                        }
                    } else {
                        println!("{} last modified time not available", file_name);
                        let _ = update_file(
                            path_buf_clone,
                            file_id,
                            port,
                            index_types_inner,
                            runtime_inner,
                            tx_clone.clone(),
                        );
                    }
                } else {
                    println!("File {} does not exist", file_name);
                    let _ = process_file(
                        path_buf_clone,
                        root_id_clone,
                        true,
                        port,
                        index_types_inner,
                        runtime_inner,
                        tx_clone.clone(),
                    );
                }
            });
        }
    });

    Ok(())
}

pub fn update_folder(
    current_path: PathBuf,
    folder_id: String,
    port: u16,
    index_types: Arc<serde_json::Value>,
    runtime: Arc<Runtime>,
    tx: tokio::sync::mpsc::Sender<utils::EmbeddingJob>,
    update_interval: u64,
) -> Result<()> {
    let url = format!("http://localhost:{}/{}", port, "getSubFolders");
    let payload = json!({ "folder_id": folder_id });
    let folder_res = post_request(&url, payload, &runtime)?;
    let subfolders = folder_res
        .get("subfolders")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("Folder ID not found"))?;

    let mut subfolder_name_ids = HashMap::new();

    for folder in subfolders {
        let folder_id = folder.get("id").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Folder ID not found"))?;
        let folder_name = folder.get("name").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("Folder name not found"))?;
        subfolder_name_ids.insert(folder_name.to_string(), folder_id.to_string());
    }

    // println!("Subfolder IDs: {:#?}", subfolder_name_ids);

    let url = format!("http://localhost:{}/{}", port, "getFolderFiles");
    let payload = json!({ "folder_id": folder_id });
    let folder_file_res = post_request(&url, payload, &runtime)?;
    let folder_files = folder_file_res
        .get("files")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("Folder ID not found"))?;

    let mut folder_file_name_ids = HashMap::new();

    for file in folder_files {
        let file_id = file.get("id").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("File ID not found"))?;
        let file_name = file.get("name").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("File name not found"))?;
        folder_file_name_ids.insert(file_name.to_string(), file_id.to_string());
    }

    // println!("Subfolder file IDs: {:#?}", folder_file_name_ids);

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

    entries.par_iter().for_each(|entry| {
        let path = entry.path();
        let path_buf = path.to_path_buf();
        let index_types_clone = Arc::clone(&index_types);
        let runtime_clone = Arc::clone(&runtime);
        let subfolder_name_ids = subfolder_name_ids.clone();
        let tx_clone = tx.clone();

        // Folder
        if path_buf.is_dir(){
            let folder_name = path_buf.file_name().unwrap().to_str().unwrap();
            let folder_id = subfolder_name_ids.get(folder_name).unwrap().to_string();
            println!("Submitting {} folder for processing", folder_name);
            if subfolder_name_ids.contains_key(folder_name){
                println!("Folder {} already exists", folder_name);
                let _ = update_folder(path_buf.clone(), folder_id.clone(), port, index_types_clone, runtime_clone, tx_clone, update_interval);
            } else {
                println!("Folder {} does not exist", folder_name);
                let _ = populate(path_buf.clone(), folder_id.clone(), port, false, index_types_clone, runtime_clone, tx_clone);
            }

        // File
        } else if path_buf.is_file() {
            // Increment thread counter
            ACTIVE_THREADS.fetch_add(1, Ordering::SeqCst);

            // Process the file using the thread pool
            let path_buf_clone = path_buf.clone();
            let folder_id_clone = folder_id.clone();
            let index_types_inner = Arc::clone(&index_types_clone);
            let runtime_inner = Arc::clone(&runtime_clone);
            let folder_file_name_ids_clone = folder_file_name_ids.clone();
            
            rayon::spawn(move || {
                // Use a guard to ensure counter is decremented when thread exits
                struct ThreadGuard;
                impl Drop for ThreadGuard {
                    fn drop(&mut self) {
                        ACTIVE_THREADS.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                let _guard = ThreadGuard;

                let file_name = path_buf_clone.file_name().unwrap().to_str().unwrap();
                
                if folder_file_name_ids_clone.contains_key(file_name){
                    let file_id = folder_file_name_ids_clone.get(file_name).unwrap().to_string();
                    let metadata = fs::metadata(&path_buf_clone).expect("Failed to get metadata");
                    if let Ok(last_modified) = metadata.modified() {
                        let date_modified = DateTime::<Utc>::from(last_modified);

                        let url = format!("http://localhost:{}/{}", port, "getFile");
                        let payload = json!({ "file_id": file_id });
                        let file_res = post_request(&url, payload, &runtime_inner).expect("File not found");
                        let file = file_res.get("file").expect("File not found");
                        let extracted_at = file.get("extracted_at").and_then(|v| v.as_str()).expect("File ID not found");
                        let date_extracted = DateTime::parse_from_rfc3339(extracted_at)
                            .expect("Failed to parse date")
                            .with_timezone(&Utc);

                        let diff_sec = date_modified.signed_duration_since(date_extracted).num_seconds();
                        if diff_sec > update_interval.try_into().unwrap() {
                            println!("File {} is out of date", file_name);
                            let _ = update_file(
                                path_buf_clone,
                                file_id,
                                port,
                                index_types_inner,
                                runtime_inner,
                                tx_clone.clone(),
                            );
                        } else {
                            println!("File {} is up to date", file_name);
                        }

                    } else {
                        println!("File {} last modified time not available", file_name);
                        let _ = update_file(
                            path_buf_clone,
                            file_id,
                            port,
                            index_types_inner,
                            runtime_inner,
                            tx_clone.clone(),
                        );
                    }
                } else {
                    println!("File {} does not exist", file_name);
                    let _ = process_file(
                        path_buf_clone,
                        folder_id_clone,
                        false,
                        port,
                        index_types_inner,
                        runtime_inner,
                        tx_clone.clone(),
                    );
                }
            });
        }
    });

    Ok(())
}

pub fn update_file(
    file_path: PathBuf,
    file_id: String,
    port: u16,
    index_types: Arc<serde_json::Value>,
    runtime: Arc<Runtime>,
    tx: tokio::sync::mpsc::Sender<utils::EmbeddingJob>,
) -> Result<()> {
    let source_code = fs::read_to_string(&file_path)?;
    let file_name = file_path.file_name().unwrap().to_str().unwrap();
    let extension = file_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("txt");
    
    if let Some(language) = get_language(&file_path) {
        // Parse file
        let mut parser = Parser::new();
        parser.set_language(&language)?;
        let tree = parser.parse(&source_code, None).unwrap();

        // Update file
        let time_now = Utc::now().to_rfc3339();
        let endpoint = "updateFile";
        let url = format!("http://localhost:{}/{}", port, endpoint);
        let payload = json!({ "file_id": file_id, "text": source_code, "extracted_at": time_now });

        // Send request to update file
        println!("\nUpdating file: {}", file_name);
        let _ = post_request(&url, payload, &runtime);

        let _ = delete_entities(file_id.to_string(), true, port, runtime.clone());

        // Process entities
        let root_node = tree.root_node();
        let mut binding = root_node.walk();
        
        // Collect children into a Vec to enable parallel iteration
        let children: Vec<Node> = root_node.children(&mut binding).collect();
        println!("Updating {} super entities from file {}", children.len(), file_name);
        
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
                                        // Increment the total chunks counter
                                        TOTAL_CHUNKS.fetch_add(chunks.len(), Ordering::SeqCst);
                                        
                                        // Process chunks in parallel
                                        // chunks.par_iter().for_each(|chunk| {
                                        //     let chunk_clone = chunk.clone();
                                        //     let entity_id_clone = entity_id.to_string();
                                            
                                        //     // Increment thread counter for embedding
                                        //     ACTIVE_THREADS.fetch_add(1, Ordering::SeqCst);
                                            
                                        //     // Use a guard to ensure counter is decremented when thread exits
                                        //     struct EmbedThreadGuard;
                                        //     impl Drop for EmbedThreadGuard {
                                        //         fn drop(&mut self) {
                                        //             ACTIVE_THREADS.fetch_sub(1, Ordering::SeqCst);
                                        //         }
                                        //     }
                                        //     let _embed_guard = EmbedThreadGuard;
                                            
                                        //     // Generate embedding
                                        //     let job = utils::EmbeddingJob {
                                        //         chunk: chunk_clone,
                                        //         entity_id: entity_id_clone,
                                        //         port,
                                        //     };
                                        //     let tx_clone = tx.clone();
                                        //     if let Err(e) = tx_clone.blocking_send(job) {
                                        //         eprintln!("Failed to send embedding job: {}", e);
                                        //     }
                                        // });
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
                tx.clone(),
            ) {
                eprintln!("Error processing entity: {}", e);
            }
        });
    // File is not supported by Tree Sitter
    } else {
        // Create file without entities
        let endpoint =  "updateFile";
        let url = format!("http://localhost:{}/{}", port, endpoint);
        let payload = json!({ "file_id": file_id, "text": source_code });

        // Send request to update file
        println!("\nUpdating unsupported file: {}", file_name);
        post_request(&url, payload, &runtime)?;
    }
    Ok(())
}

pub fn delete_entities(
    parent_id: String,
    is_super: bool,
    port: u16,
    runtime: Arc<Runtime>,
) -> Result<()> {
    if is_super {
         // Get Sub Entities
        let endpoint = "getFileEntities";
        let url = format!("http://localhost:{}/{}", port, endpoint);
        let payload = json!({ "file_id": parent_id });
        let response = post_request(&url, payload, &runtime)?;
        let super_entities = response
            .get("entity")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow::anyhow!("Super entities not found"))?;

        let super_entity_ids: Vec<String> = super_entities
            .iter()
            .map(|v| v.get("id").and_then(|v| v.as_str()).unwrap().to_string())
            .collect();

        // println!("Deleting {} super entities", super_entity_ids.len());
        super_entity_ids.par_iter().for_each(|super_entity_id| {
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

            let runtime_clone = Arc::clone(&runtime);

            let _ = delete_entities(super_entity_id.to_string(), false, port, runtime_clone);

            // Delete super entity
            let endpoint = "deleteSuperEntity";
            let url = format!("http://localhost:{}/{}", port, endpoint);
            let payload = json!({ "entity_id": super_entity_id });
            let _ = post_request(&url, payload, &runtime);
        });
    } else {
         // Get Sub Entities
        let endpoint = "getSubEntities";
        let url = format!("http://localhost:{}/{}", port, endpoint);
        let payload = json!({ "entity_id": parent_id });
        let response = post_request(&url, payload, &runtime)?;
        let sub_entities = response
            .get("entities")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow::anyhow!("Sub entities not found"))?;

        let sub_entity_ids: Vec<String> = sub_entities
            .iter()
            .map(|v| v.get("id").and_then(|v| v.as_str()).unwrap().to_string())
            .collect();

        // println!("Deleting {} sub entities", sub_entity_ids.len());
        sub_entity_ids.par_iter().for_each(|sub_entity_id| {
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

            let runtime_clone = Arc::clone(&runtime);

            let _ = delete_entities(sub_entity_id.to_string(), false, port, runtime_clone);

            // Delete sub entity
            let endpoint = "deleteSubEntity";
            let url = format!("http://localhost:{}/{}", port, endpoint);
            let payload = json!({ "entity_id": sub_entity_id });
            let _ = post_request(&url, payload, &runtime);
        });
    }
    Ok(())
}

pub fn ingestion(
    root_path: PathBuf,
    port: u16,
    _concur_limit: usize,
    runtime: Arc<Runtime>,
    tx: tokio::sync::mpsc::Sender<utils::EmbeddingJob>,
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
        tx,
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
    tx: tokio::sync::mpsc::Sender<utils::EmbeddingJob>,
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
                        let tx_clone = tx.clone();
                        
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
                                tx_clone.clone(),
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
            let tx_clone = tx.clone();
            
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
                    tx_clone.clone(),
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
    tx: tokio::sync::mpsc::Sender<utils::EmbeddingJob>,
) -> Result<()> {
    // Read file contents
    let source_code = fs::read_to_string(&file_path)?;
    let file_name = file_path.file_name().unwrap().to_str().unwrap();
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
        // println!("Processing {} super entities from file {}", children.len(), file_name);
        
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
                                        // Increment the total chunks counter
                                        TOTAL_CHUNKS.fetch_add(chunks.len(), Ordering::SeqCst);
                                        
                                        // Process chunks in parallel
                                        // chunks.par_iter().for_each(|chunk| {
                                        //     let chunk_clone = chunk.clone();
                                        //     let entity_id_clone = entity_id.to_string();
                                            
                                        //     // Increment thread counter for embedding
                                        //     ACTIVE_THREADS.fetch_add(1, Ordering::SeqCst);
                                            
                                        //     // Use a guard to ensure counter is decremented when thread exits
                                        //     struct EmbedThreadGuard;
                                        //     impl Drop for EmbedThreadGuard {
                                        //         fn drop(&mut self) {
                                        //             ACTIVE_THREADS.fetch_sub(1, Ordering::SeqCst);
                                        //         }
                                        //     }
                                        //     let _embed_guard = EmbedThreadGuard;
                                            
                                        //     // Generate embedding
                                        //     let job = utils::EmbeddingJob {
                                        //         chunk: chunk_clone,
                                        //         entity_id: entity_id_clone,
                                        //         port,
                                        //     };
                                        //     let tx_clone = tx.clone();
                                        //     if let Err(e) = tx_clone.blocking_send(job) {
                                        //         eprintln!("Failed to send embedding job: {}", e);
                                        //     }
                                        // });
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
                tx.clone(),
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
    tx: tokio::sync::mpsc::Sender<utils::EmbeddingJob>,
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
                    tx.clone(),
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
                        // Increment the total chunks counter
                        TOTAL_CHUNKS.fetch_add(chunks.len(), Ordering::SeqCst);
                        // println!("chunks length: {}", chunks.len());
                        // println!("Chunking entity: {}", code_entity.text);

                        // Process chunks in parallel using rayon
                        // chunks.par_iter().for_each(|chunk| {
                        //     let chunk_clone = chunk.clone();
                        //     let entity_id_clone = entity_id.clone();

                        //     // Increment thread counter
                        //     ACTIVE_THREADS.fetch_add(1, Ordering::SeqCst);

                        //     // Send embedding job to async background worker via mpsc channel
                        //     let job = utils::EmbeddingJob {
                        //         chunk: chunk_clone,
                        //         entity_id: entity_id_clone,
                        //         port,
                        //     };
                        //     let tx_clone = tx.clone();
                        //     if let Err(e) = tx_clone.blocking_send(job) {
                        //         eprintln!("Failed to send embedding job: {}", e);
                        //     }

                        //     // Decrement counters
                        //     ACTIVE_THREADS.fetch_sub(1, Ordering::SeqCst);
                        // });
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
                                tx.clone(),
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
