mod utils;

use anyhow::Result;
use ignore::WalkBuilder;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::sync::Semaphore;
use tree_sitter::Parser;
use std::future::Future;
use std::pin::Pin;

use utils::{chunk_entity, embed_entity, extract_entities_recursive, get_language, post_request, CodeEntity};

const DEFAULT_IGNORE_PATTERNS: &[&str] = &[".git/"];
const MAX_DEPTH: usize = 2;

#[tokio::main]
async fn main() -> Result<()> {
    let root_path = PathBuf::from("sample").canonicalize()?;
    let port = 6969;

    println!(
        "Starting ingestion for directory: {}",
        root_path.display()
    );

    let root_name = root_path.file_name().unwrap().to_str().unwrap();
    let url = format!("http://localhost:{}/createRoot", port);
    let payload = json!({ "name": root_name });
    let root_response = post_request(&url, payload).await?;
    let root_id = root_response
        .get("root")
        .and_then(|f| f.get(0))
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Root ID not found"))?;

    println!("Root created");

    let semaphore = Arc::new(Semaphore::new(10));

    populate(
        root_path,
        root_id.to_string(),
        port,
        semaphore,
        true, // is_super
    )
    .await?;

    println!("Ingestion finished");
    Ok(())
}

fn populate(
    current_path: PathBuf,
    parent_id: String,
    port: u16,
    semaphore: Arc<Semaphore>,
    is_super: bool,
) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
        let mut tasks = Vec::new();
        let mut walker_builder = WalkBuilder::new(&current_path);
        walker_builder.max_depth(Some(1));
        for pattern in DEFAULT_IGNORE_PATTERNS {
            walker_builder.add_ignore(pattern);
        }

        for result in walker_builder.build() {
            let entry = match result {
                Ok(entry) => entry,
                Err(e) => {
                    eprintln!("Error walking directory: {}", e);
                    continue;
                }
            };

            let path = entry.path();
            if path == current_path {
                continue;
            }

            let semaphore_clone = Arc::clone(&semaphore);
            let parent_id_clone = parent_id.clone();
            let path_buf = path.to_path_buf();

            tasks.push(tokio::spawn(async move {
                let _permit = semaphore_clone.clone().acquire_owned().await?;
                if path_buf.is_dir() {
                    let folder_name = path_buf.file_name().unwrap().to_str().unwrap();
                    let endpoint = if is_super { "createSuperFolder" } else { "createSubFolder" };
                    let url = format!("http://localhost:{}/{}", port, endpoint);
                    let payload = if is_super {
                        json!({ "name": folder_name, "root_id": parent_id_clone })
                    } else {
                        json!({ "name": folder_name, "folder_id": parent_id_clone })
                    };

                    println!("Submitting {} folder for processing", folder_name);

                    if let Ok(res) = post_request(&url, payload).await {
                        if let Some(folder_id) = res.get("folder").and_then(|f| f.get(0)).and_then(|v| v.get("id")).and_then(|v| v.as_str()) {
                            populate(path_buf, folder_id.to_string(), port, semaphore_clone, false).await?;
                        }
                    } else {
                        eprintln!("Failed to create folder: {}", folder_name);
                    }
                } else if path_buf.is_file() {
                    process_file(path_buf, parent_id_clone, is_super, port).await?;
                }
                Ok::<(), anyhow::Error>(())
            }));
        }

        for task in tasks {
            task.await??;
        }

        Ok(())
    })
}

async fn process_file(
    file_path: PathBuf,
    parent_id: String,
    is_super: bool,
    port: u16,
) -> Result<()> {
    let source_code = fs::read_to_string(&file_path).await?;

    if let Some(language) = get_language(&file_path) {
        let mut parser = Parser::new();
        parser.set_language(language)?;
        let tree = parser.parse(&source_code, None).unwrap();
        let tree_dict = extract_entities_recursive(tree.root_node(), &source_code);

        let file_name = file_path.file_name().unwrap().to_str().unwrap();
        let endpoint = if is_super { "createSuperFile" } else { "createFile" };
        let url = format!("http://localhost:{}/{}", port, endpoint);

        let file_type = if is_super { "super" } else { "sub" };
        println!("Processing {} file: {}", file_type, file_name);

        let payload = if is_super {
            json!({ "name": file_name, "root_id": parent_id, "text": source_code })
        } else {
            json!({ "name": file_name, "folder_id": parent_id, "text": source_code })
        };

        let file_response = post_request(&url, payload).await?;
        let file_id = file_response.get("file").and_then(|f| f.get(0)).and_then(|v| v.get("id")).and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("File ID not found"))?;

        println!("Processing {} super entities from file {}", tree_dict.len(), file_name);
        for entity in tree_dict.into_iter() {
            process_entity(entity, file_id.to_string(), port, true, 0).await?;
        }
    } else {
        println!("Ignored: {}", file_path.display());
    }
    Ok(())
}

fn process_entity(
    entity: CodeEntity,
    parent_id: String,
    port: u16,
    is_super: bool,
    depth: usize,
) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
    Box::pin(async move {
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

        let entity_response = post_request(&url, payload).await?;
        let entity_id = entity_response.get("entity").and_then(|f| f.get(0)).and_then(|v| v.get("id")).and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Entity ID not found"))?;

        if is_super {
            let chunks = chunk_entity(&entity.text).await;
            for chunk in chunks {
                let embedding = embed_entity(chunk).await;
                let embed_endpoint = "embedSuperEntity";
                let payload = json!({
                    "entity_id": entity_id,
                    "vector": embedding,
                });
                let embed_url = format!("http://localhost:{}/{}", port, embed_endpoint);
                post_request(&embed_url, payload).await?;
            }
        }

        if depth < MAX_DEPTH {
            for child in entity.children.into_iter() {
                process_entity(child, entity_id.to_string(), port, false, depth + 1).await?;
            }
        }
        
        Ok(())
    })
}
