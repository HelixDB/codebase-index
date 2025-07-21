use anyhow::Result;
use ignore::WalkBuilder;
use rayon::prelude::*;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc
};
use tokio::runtime::Runtime;
use chrono::{DateTime, Utc};


// Import from our modules
use crate::utils::{
    post_request, delete_folder, delete_files, EmbeddingJob,
    ACTIVE_THREADS
};
use crate::queries::{get_root_folders, get_root_files, get_sub_folders, get_folder_files};

// Forward declarations for functions that will be moved from ingestion
use crate::ingestion::{populate, process_file, ingest_entities, process_unsupported_file};
use crate::utils::{get_language, delete_entities, chunk_entity, TOTAL_CHUNKS};
use tree_sitter::Parser;

pub fn update(
    root_path: PathBuf,
    root_id: String,
    port: u16,
    runtime: Arc<Runtime>,
    tx: tokio::sync::mpsc::Sender<EmbeddingJob>,
    update_interval: u64,
) -> Result<()> {    
    // Load index types
    let index_types = fs::read_to_string("index-types.json")?;
    let index_types: serde_json::Value = serde_json::from_str(&index_types)?;
    let index_types = Arc::new(index_types);

    // Check if root exists
    let url = format!("http://localhost:{}/{}", port, "getRootById");
    let root_res = post_request(&url, json!({ "root_id": root_id }), &runtime)?;
    let root = root_res
        .get("root")
        .and_then(|v| v.get("name"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Root not found"))?;

    let root_name = root_path.file_name().unwrap().to_str().unwrap();

    if root != root_name {
        return Err(anyhow::anyhow!("Root name does not match"));
    }

    let root_folder_name_ids = get_root_folders(root_id.clone(), &runtime, port)?;
    // println!("Root folder IDs: {:#?}", root_folder_name_ids);

    let root_file_name_ids = get_root_files(root_id.clone(), &runtime, port)?;
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
                // println!("Folder {} already exists", folder_name);
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
            let index_types_inner = Arc::clone(&index_types_clone);
            let runtime_inner = Arc::clone(&runtime_clone);
            
            // Use a guard to ensure counter is decremented when thread exits
            struct ThreadGuard;
            impl Drop for ThreadGuard {
                fn drop(&mut self) {
                    ACTIVE_THREADS.fetch_sub(1, Ordering::SeqCst);
                }
            }
            let _guard = ThreadGuard;

            let file_name = path_buf_clone.file_name().unwrap().to_str().unwrap();
            
            if root_file_name_ids.contains_key(file_name){
                let file_id = root_file_name_ids.get(file_name).unwrap().0.to_string();
                let file_extracted_at = root_file_name_ids.get(file_name).unwrap().1.to_string();
                let metadata = fs::metadata(&path_buf_clone).expect("Failed to get metadata");
                if let Ok(last_modified) = metadata.modified() {
                    let date_modified = DateTime::<Utc>::from(last_modified);
                    
                    let date_extracted = DateTime::parse_from_rfc3339(&file_extracted_at)
                        .expect("Failed to parse date")
                        .with_timezone(&Utc);

                    let diff_sec = date_modified.signed_duration_since(date_extracted).num_seconds();
                    if diff_sec > update_interval.try_into().unwrap() {
                        println!("File {} is out of date", file_name);
                        let _ = update_file(
                            path_buf_clone,file_id,port,
                            index_types_inner,runtime_inner,tx_clone
                        );
                    }
                } else {
                    println!("File {} last modified time not available", file_name);
                    let _ = update_file(
                        path_buf_clone,file_id,port,
                        index_types_inner,runtime_inner,tx_clone
                    );
                }
            } else {
                println!("File {} does not exist", file_name);
                let _ = process_file(
                    path_buf_clone, root_id.clone(), true, 
                    port, index_types_inner, runtime_inner, tx_clone
                );
            }
        }
    });

    // Find folders that are not in the index
    let unseen_folders = root_folder_name_ids.keys()
        .filter(|folder_name| !entries.iter().any(|entry| entry.path().file_name().unwrap().to_str().unwrap().to_string() == folder_name.to_string()))
        .collect::<Vec<_>>();

    unseen_folders.par_iter().for_each(|folder_name| {
        let folder_id = root_folder_name_ids.get(&folder_name.to_string()).unwrap().to_string();
        let runtime_clone = Arc::clone(&runtime);
        let _ = delete_folder(folder_id, port, runtime_clone);
    });

    let unseen_files = root_file_name_ids.keys()
        .filter(|file_name| !entries.iter().any(|entry| entry.path().file_name().unwrap().to_str().unwrap().to_string() == file_name.to_string()))
        .map(|s| s.to_string())
        .collect::<Vec<_>>();

    delete_files(unseen_files.clone(), root_file_name_ids.clone(), runtime.clone(), port)?;

    Ok(())
}

pub fn update_folder(
    current_path: PathBuf,
    folder_id: String,
    port: u16,
    index_types: Arc<serde_json::Value>,
    runtime: Arc<Runtime>,
    tx: tokio::sync::mpsc::Sender<EmbeddingJob>,
    update_interval: u64,
) -> Result<()> {
    let subfolder_name_ids  = get_sub_folders(folder_id.clone(), &runtime, port)?;
    // println!("Subfolder IDs: {:#?}", subfolder_name_ids);

    let folder_file_name_ids = get_folder_files(folder_id.clone(), &runtime, port)?;
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
            if subfolder_name_ids.contains_key(folder_name){
                // println!("Folder {} already exists", folder_name);
                let folder_id = subfolder_name_ids.get(folder_name).unwrap().to_string();
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
            let index_types_inner = Arc::clone(&index_types_clone);
            let runtime_inner = Arc::clone(&runtime_clone);
            
            // Use a guard to ensure counter is decremented when thread exits
            struct ThreadGuard;
            impl Drop for ThreadGuard {
                fn drop(&mut self) {
                    ACTIVE_THREADS.fetch_sub(1, Ordering::SeqCst);
                }
            }
            let _guard = ThreadGuard;

            let file_name = path_buf_clone.file_name().unwrap().to_str().unwrap();
            
            if folder_file_name_ids.contains_key(file_name){
                let file_id = folder_file_name_ids.get(file_name).unwrap().0.to_string();
                let file_extracted_at = folder_file_name_ids.get(file_name).unwrap().1.to_string();
                let metadata = fs::metadata(&path_buf_clone).expect("Failed to get metadata");
                if let Ok(last_modified) = metadata.modified() {
                    let date_modified = DateTime::<Utc>::from(last_modified);
                    let date_extracted = DateTime::parse_from_rfc3339(&file_extracted_at)
                        .expect("Failed to parse date")
                        .with_timezone(&Utc);

                    let diff_sec = date_modified.signed_duration_since(date_extracted).num_seconds();
                    if diff_sec > update_interval.try_into().unwrap() {
                        println!("File {} is out of date", file_name);
                        let _ = update_file(
                            path_buf_clone, file_id, port,
                            index_types_inner, runtime_inner, tx_clone,
                        );
                    }
                } else {
                    println!("File {} last modified time not available", file_name);
                    let _ = update_file(
                        path_buf_clone, file_id, port,
                        index_types_inner, runtime_inner, tx_clone,
                    );
                }
            } else {
                println!("File {} does not exist", file_name);
                let _ = process_file(
                    path_buf_clone, folder_id.clone(), false, port,
                    index_types_inner, runtime_inner, tx_clone
                );
            }
        }
    });

    // Find folders that are not in the index
    let unseen_folders = subfolder_name_ids.keys()
        .filter(|folder_name| !entries.iter().any(|entry| entry.path().file_name().unwrap().to_str().unwrap().to_string() == folder_name.to_string()))
        .collect::<Vec<_>>();

    unseen_folders.par_iter().for_each(|folder_name| {
        let folder_id = subfolder_name_ids.get(&folder_name.to_string()).unwrap().to_string();
        let runtime_clone = Arc::clone(&runtime);
        let _ = delete_folder(folder_id, port, runtime_clone);
    });

    let unseen_files = folder_file_name_ids.keys()
        .filter(|file_name| !entries.iter().any(|entry| entry.path().file_name().unwrap().to_str().unwrap().to_string() == file_name.to_string()))
        .map(|s| s.to_string())
        .collect::<Vec<_>>();

    delete_files(unseen_files.clone(), folder_file_name_ids.clone(), runtime.clone(), port)?;

    Ok(())
}

pub fn update_file(
    file_path: PathBuf,
    file_id: String,
    port: u16,
    index_types: Arc<serde_json::Value>,
    runtime: Arc<Runtime>,
    tx: tokio::sync::mpsc::Sender<EmbeddingJob>,
) -> Result<()> {
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
    
    if let Some(language) = get_language(&file_path) {
        // Parse file
        let mut parser = Parser::new();
        parser.set_language(&language)?;
        let tree = parser.parse(&source_code, None).unwrap();

        // Update file
        let time_now = Utc::now().to_rfc3339();
        let url = format!("http://localhost:{}/{}", port, "updateFile");
        let payload = json!({ "file_id": file_id, "text": source_code, "extracted_at": time_now });

        // Send request to update file
        println!("\nUpdating file: {}", file_name);
        let _ = post_request(&url, payload, &runtime);

        let _ = delete_entities(file_id.to_string(), true, port, runtime.clone());

        // Process entities
        let root_node = tree.root_node();
        let mut binding = root_node.walk();
        
        // Collect children into a Vec to enable parallel iteration
        let children: Vec<tree_sitter::Node> = root_node.children(&mut binding).collect();
        println!("Updating {} super entities from file {}", children.len(), file_name);
        
        // Use a shared counter for order to ensure consistent ordering
        let order_counter = Arc::new(AtomicUsize::new(1));
        
        // Process entities in parallel
        ingest_entities(&children, &source_code, file_id.to_string(), port,
            extension.to_string(), index_types, &runtime, &tx, &order_counter)?;
    // File is not supported by Tree Sitter
    } else {
        // Create file without entities
        let endpoint =  "updateFile";
        let url = format!("http://localhost:{}/{}", port, endpoint);
        let payload = json!({ "file_id": file_id, "text": source_code });

        // Send request to update file
        println!("\nUpdating unsupported file: {}", file_name);
        post_request(&url, payload, &runtime)?;

        let _ = delete_entities(file_id.to_string(), true, port, runtime.clone());

        let chunks = chunk_entity(&source_code).unwrap();
        let order_counter = Arc::new(AtomicUsize::new(1));
        TOTAL_CHUNKS.fetch_add(chunks.len(), Ordering::SeqCst);

        process_unsupported_file(chunks, file_id.to_string(), port, order_counter, tx, runtime)?;
    }
    
    Ok(())
}