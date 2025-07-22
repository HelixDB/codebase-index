use anyhow::Result;
use ignore::WalkBuilder;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc
};
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;
use chrono::{DateTime, Utc};
use async_recursion::async_recursion;


// Import from our modules
use crate::utils::{
    post_request_async, delete_folder, delete_files, EmbeddingJob,
    TOTAL_CHUNKS
};
use crate::queries::{get_root_folders, get_root_files, get_sub_folders, get_folder_files};

// Forward declarations for functions that will be moved from ingestion
use crate::ingestion::{populate, process_file, ingest_entities, process_unsupported_file, build_owned_nodes};
use crate::utils::{get_language, delete_entities, chunk_entity};
use tree_sitter::Parser;

#[async_recursion]
pub async fn update(
    root_path: PathBuf,
    root_id: String,
    port: u16,
    tx: Sender<EmbeddingJob>,
    update_interval: u64,
) -> Result<()> {    
    // Load index types
    let index_types = fs::read_to_string("index-types.json")?;
    let index_types: serde_json::Value = serde_json::from_str(&index_types)?;
    let index_types = Arc::new(index_types);

    // Check if root exists
    let url = format!("http://localhost:{}/{}", port, "getRootById");
    let root_res = post_request_async(&url, json!({ "root_id": root_id })).await?;
    let root = root_res
        .get("root")
        .and_then(|v| v.get("name"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Root not found"))?;

    let root_name = root_path.file_name().unwrap().to_str().unwrap();

    if root != root_name {
        return Err(anyhow::anyhow!("Root name does not match"));
    }

    let root_folder_name_ids = get_root_folders(root_id.clone(), port).await?;
    // println!("Root folder IDs: {:#?}", root_folder_name_ids);

    let root_file_name_ids = get_root_files(root_id.clone(), port).await?;
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

    let tasks: Vec<JoinHandle<Result<()>>> = entries.clone().clone().into_iter().map(|entry| {
        let path_buf = entry.path().to_path_buf();
        let index_types_clone = index_types.clone();
        let root_folder_name_ids_clone = root_folder_name_ids.clone();
        let root_file_name_ids_clone = root_file_name_ids.clone();
        let root_id_clone = root_id.clone();
        let tx_clone = tx.clone();

        tokio::spawn(async move {
            // Folder
            if path_buf.is_dir(){
                let folder_name = path_buf.file_name().unwrap().to_str().unwrap();
                if root_folder_name_ids_clone.contains_key(folder_name){
                    // println!("Folder {} already exists", folder_name);
                    let folder_id = root_folder_name_ids_clone.get(folder_name).unwrap().to_string();
                    let _ = Box::pin(update_folder(path_buf.clone(), folder_id.clone(), port, index_types_clone, tx_clone, update_interval)).await;
                } else {
                    println!("Folder {} does not exist", folder_name);
                    let _ = populate(path_buf.clone(), root_id_clone, port, true, index_types_clone, tx_clone).await;
                }
                Ok(())

            // File
            } else if path_buf.is_file() {
                let file_name = path_buf.file_name().unwrap().to_str().unwrap();
                
                if root_file_name_ids_clone.contains_key(file_name){
                    let file_id = root_file_name_ids_clone.get(file_name).unwrap().0.to_string();
                    let file_extracted_at = root_file_name_ids_clone.get(file_name).unwrap().1.to_string();
                    let metadata = fs::metadata(&path_buf).expect("Failed to get metadata");
                    if let Ok(last_modified) = metadata.modified() {
                        let date_modified = DateTime::<Utc>::from(last_modified);
                        
                        let date_extracted = DateTime::parse_from_rfc3339(&file_extracted_at)
                            .expect("Failed to parse date")
                            .with_timezone(&Utc);

                        let diff_sec = date_modified.signed_duration_since(date_extracted).num_seconds();
                        if diff_sec > update_interval.try_into().unwrap() {
                            println!("File {} is out of date", file_name);
                            let _ = update_file(
                                path_buf,file_id,port,
                                index_types_clone,tx_clone
                            ).await;
                        }
                    } else {
                        println!("File {} last modified time not available", file_name);
                        let _ = update_file(
                            path_buf,file_id,port,
                            index_types_clone,tx_clone
                        ).await;
                    }
                } else {
                    println!("File {} does not exist", file_name);
                    let _ = process_file(
                        path_buf, root_id_clone, true, 
                        port, index_types_clone, tx_clone
                    ).await;
                }
                Ok(())
            } else {
                Ok(())
            }
        })
    }).collect();

    for task in tasks {
        task.await??;
    }

    // Find folders that are not in the index
    let unseen_folders: Vec<String> = root_folder_name_ids.keys()
        .filter(|folder_name| !entries.clone().iter().any(|entry| entry.path().file_name().unwrap().to_str().unwrap() == **folder_name))
        .cloned()
        .collect();

    let delete_folder_tasks: Vec<JoinHandle<Result<()>>> = unseen_folders.into_iter().map(|folder_name| {
        let folder_id = root_folder_name_ids.get(&folder_name).unwrap().to_string().clone();

        tokio::spawn(async move {
            delete_folder(folder_id, port).await
        })
    }).collect();

    for task in delete_folder_tasks {
        task.await??;
    }

    let unseen_files = root_file_name_ids.keys()
        .filter(|file_name| !entries.iter().any(|entry| entry.path().file_name().unwrap().to_str().unwrap() == **file_name))
        .cloned()
        .collect::<Vec<_>>();

    delete_files(unseen_files, root_file_name_ids, port).await?;

    Ok(())
}

#[async_recursion]
pub async fn update_folder(
    current_path: PathBuf,
    folder_id: String,
    port: u16,
    index_types: Arc<serde_json::Value>,
    tx: Sender<EmbeddingJob>,
    update_interval: u64,
) -> Result<()> {
    let subfolder_name_ids  = get_sub_folders(folder_id.clone(), port).await?;
    // println!("Subfolder IDs: {:#?}", subfolder_name_ids);

    let folder_file_name_ids = get_folder_files(folder_id.clone(), port).await?;
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

    let tasks: Vec<JoinHandle<Result<()>>> = entries.clone().clone().into_iter().map(|entry| {
        let path_buf = entry.path().to_path_buf();
        let index_types_clone = index_types.clone();
        let subfolder_name_ids_clone = subfolder_name_ids.clone();
        let folder_file_name_ids_clone = folder_file_name_ids.clone();
        let folder_id_clone = folder_id.clone();
        let tx_clone = tx.clone();

        tokio::spawn(async move {
            // Folder
            if path_buf.is_dir(){
                let folder_name = path_buf.file_name().unwrap().to_str().unwrap();
                if subfolder_name_ids_clone.contains_key(folder_name){
                    // println!("Folder {} already exists", folder_name);
                    let sub_folder_id = subfolder_name_ids_clone.get(folder_name).unwrap().to_string();
                    let _ = Box::pin(update_folder(path_buf.clone(), sub_folder_id, port, index_types_clone, tx_clone, update_interval)).await;
                } else {
                    println!("Folder {} does not exist", folder_name);
                    let _ = populate(path_buf.clone(), folder_id_clone, port, false, index_types_clone, tx_clone).await;
                }
                Ok(())

            // File
            } else if path_buf.is_file() {
                let file_name = path_buf.file_name().unwrap().to_str().unwrap();
                
                if folder_file_name_ids_clone.contains_key(file_name){
                    let file_id = folder_file_name_ids_clone.get(file_name).unwrap().0.to_string();
                    let file_extracted_at = folder_file_name_ids_clone.get(file_name).unwrap().1.to_string();
                    let metadata = fs::metadata(&path_buf).expect("Failed to get metadata");
                    if let Ok(last_modified) = metadata.modified() {
                        let date_modified = DateTime::<Utc>::from(last_modified);
                        let date_extracted = DateTime::parse_from_rfc3339(&file_extracted_at)
                            .expect("Failed to parse date")
                            .with_timezone(&Utc);

                        let diff_sec = date_modified.signed_duration_since(date_extracted).num_seconds();
                        if diff_sec > update_interval.try_into().unwrap() {
                            println!("File {} is out of date", file_name);
                            let _ = update_file(
                                path_buf, file_id, port,
                                index_types_clone, tx_clone,
                            ).await;
                        }
                    } else {
                        println!("File {} last modified time not available", file_name);
                        let _ = update_file(
                            path_buf, file_id, port,
                            index_types_clone, tx_clone,
                        ).await;
                    }
                } else {
                    println!("File {} does not exist", file_name);
                    let _ = process_file(
                        path_buf, folder_id_clone, false, port,
                        index_types_clone, tx_clone
                    ).await;
                }
                Ok(())
            } else {
                Ok(())
            }
        })
    }).collect();

    for task in tasks {
        task.await??;
    }

    // Find folders that are not in the index
    let unseen_folders: Vec<String> = subfolder_name_ids.keys()
        .filter(|folder_name| !entries.clone().iter().any(|entry| entry.path().file_name().unwrap().to_str().unwrap() == **folder_name))
        .cloned()
        .collect();

    let delete_folder_tasks: Vec<JoinHandle<Result<()>>> = unseen_folders.into_iter().map(|folder_name| {
        let folder_id_clone = subfolder_name_ids.get(&folder_name).unwrap().to_string().clone();

        tokio::spawn(async move {
            delete_folder(folder_id_clone, port).await
        })
    }).collect();

    for task in delete_folder_tasks {
        task.await??;
    }

    let unseen_files = folder_file_name_ids.keys()
        .filter(|file_name| !entries.iter().any(|entry| entry.path().file_name().unwrap().to_str().unwrap() == **file_name))
        .cloned()
        .collect::<Vec<_>>();

    delete_files(unseen_files, folder_file_name_ids, port).await?;

    Ok(())
}

pub async fn update_file(
    file_path: PathBuf,
    file_id: String,
    port: u16,
    index_types: Arc<serde_json::Value>,
    tx: Sender<EmbeddingJob>,
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
        let _ = post_request_async(&url, payload).await;

        let _ = delete_entities(file_id.to_string(), true, port).await;

        // Process entities
        let root_node = tree.root_node();
        let owned_nodes = build_owned_nodes(root_node, &source_code);
        ingest_entities(owned_nodes, file_id.to_string(), port, extension.to_string(), index_types, tx).await?;
    // File is not supported by Tree Sitter
    } else {
        // Create file without entities
        let endpoint =  "updateFile";
        let url = format!("http://localhost:{}/{}", port, endpoint);
        let payload = json!({ "file_id": file_id, "text": source_code });

        // Send request to update file
        println!("\nUpdating unsupported file: {}", file_name);
        post_request_async(&url, payload).await?;

        let _ = delete_entities(file_id.to_string(), true, port).await;

        let chunks = chunk_entity(&source_code).unwrap();
        let order_counter = Arc::new(AtomicUsize::new(1));
        TOTAL_CHUNKS.fetch_add(chunks.len(), Ordering::SeqCst);

        process_unsupported_file(chunks, file_id.to_string(), port, order_counter, tx).await?;
    }
    
    Ok(())
}