mod utils;
mod queries;
mod updater;
mod ingestion;

// External crates
use anyhow::Result;
use serde_json::json;
use std::env;
use std::path::PathBuf;
use std::time::Instant;
use std::io;
use std::io::Write;
use dotenv;
use tokio_stream;
use futures::StreamExt;
use clearscreen;
use std::sync::atomic::Ordering;
use indicatif::{ProgressBar, ProgressStyle};

// Internal utility functions
use utils::{
    embed_entity_async, post_request_async, EmbeddingJob,
    TOTAL_CHUNKS,
    PENDING_EMBEDDINGS, COMPLETED_EMBEDDINGS,
};

use updater::update;
use ingestion::ingestion;

// Remove embedding_wait_thread function entirely

async fn async_main() {
    clear_screen();
    let args: Vec<String> = env::args().collect();

    let default_port = 6969;
    
    // Get arguments
    let path: String = if args.len() > 1 { args[1].clone() } else { "sample".to_string() };
    let port: u16 = if args.len() > 2 { args[2].parse::<u16>().unwrap() } else { default_port };
    let channel_buffer_size = 1000;

    println!("\nConnecting to Helix instance at port {}", port);

    dotenv::dotenv().ok();
    
    let (tx, rx) = tokio::sync::mpsc::channel::<EmbeddingJob>(channel_buffer_size);

    // Spawn the async background task for embedding jobs
    tokio::spawn(async move {
        // Set concurrent embeddings to better utilize our rate limit
        let max_concurrent_embeddings = 100;
        
        // Create a stream from the channel
        let mut job_stream = tokio_stream::wrappers::ReceiverStream::new(rx)
            .map(|job| {
                async move {
                    let EmbeddingJob { chunk, entity_id, port } = job;
                    if !chunk.is_empty() {
                        PENDING_EMBEDDINGS.fetch_add(1, Ordering::SeqCst);
                        match embed_entity_async(chunk).await {
                            Ok(embedding) => {
                                let url = format!("http://localhost:{}/{}", port, "embedSuperEntity");
                                let payload = json!({"entity_id": entity_id,"vector": embedding,});
                                if let Err(e) = post_request_async(&url, payload).await {
                                    eprintln!("Failed to post embedding: {}", e);
                                }
                                COMPLETED_EMBEDDINGS.fetch_add(1, Ordering::SeqCst);
                            }
                            Err(e) => {
                                eprintln!("Failed to embed chunk: {}", e);
                            }
                        }
                    }
                }
            })
            .buffer_unordered(max_concurrent_embeddings);
        
        // Process the stream
        while let Some(_) = job_stream.next().await {}
    });

    let mut root_id = String::new();

    loop {
        root_id = parse_user_input(root_id.clone(), path.clone(), port, tx.clone()).await.unwrap();
        if root_id == "EXIT" {
            break;
        }
    }
}

async fn parse_user_input(root_id: String, path: String, port: u16, tx: tokio::sync::mpsc::Sender<EmbeddingJob>) -> Result<String> {
    let path_buf = PathBuf::from(path.clone());
    let root_name = path_buf.file_name().unwrap().to_str().unwrap();
    println!("\nWhat would you like to do?\n");
    println!("1 : Ingest {}", &root_name);
    println!("2 : Update {}", &root_name);
    println!("3 : Exit");
    
    io::stdout().flush().unwrap();
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    let input = input.trim().to_string();
    let start_time = Instant::now();
    if input == "1" {
        let root_id = ingestion(
            path_buf.canonicalize().expect("Failed to canonicalize path"),
            port,
            tx.clone(),
        ).await;

        clear_screen();

        println!("\nTotal chunks processed: {}", TOTAL_CHUNKS.load(Ordering::SeqCst));
        println!("\nIngestion finished in {} seconds", start_time.elapsed().as_secs());
        wait_for_embeddings(start_time).await;
        TOTAL_CHUNKS.store(0, Ordering::SeqCst);

        return Ok(root_id.unwrap().to_string());
    } else if input == "2" {
        clear_screen();
        let root_ids = get_root_ids(port).await?;
        if root_ids.contains(&root_id) {
            println!("\nUpdating index...");
            let _ = update(
                path_buf.canonicalize().unwrap(), root_id.clone(), 
                port, tx.clone(), 5
            ).await;
            println!("\nUpdate finished in {} seconds", start_time.elapsed().as_secs());
            wait_for_embeddings(start_time).await;
            TOTAL_CHUNKS.store(0, Ordering::SeqCst);
            return Ok(root_id);
        } else {
            println!("\nNo root found");
            return Ok(root_id);
        }
    } else if input == "3" {
        clear_screen();
        return Ok("EXIT".to_string());
    }

    clear_screen();
    println!("Invalid input");
    return Ok(root_id);
}

async fn wait_for_embeddings(start_time: Instant) {
    use tokio::time::{sleep, Duration};
    println!("Waiting for all embedding jobs to complete...");
    if PENDING_EMBEDDINGS.load(Ordering::SeqCst) > 0 {
        let start_amount = PENDING_EMBEDDINGS.load(Ordering::SeqCst);
        let bar = ProgressBar::new(start_amount as u64);
        bar.set_style(ProgressStyle::with_template("[{elapsed_precise}] {wide_bar} {pos}/{len} ({per_sec}, ETA: {eta})").unwrap());
        let mut last_completed = 0;
        while COMPLETED_EMBEDDINGS.load(Ordering::SeqCst) < PENDING_EMBEDDINGS.load(Ordering::SeqCst) {
            sleep(Duration::from_millis(100)).await;
            let completed = COMPLETED_EMBEDDINGS.load(Ordering::SeqCst);
            let pending = PENDING_EMBEDDINGS.load(Ordering::SeqCst);
            if pending > start_amount {
                bar.set_length(pending as u64);
            }
            if completed > last_completed {
                let diff = completed - last_completed;
                bar.inc(diff as u64);
                last_completed = completed;
            }
        }
        bar.finish();
    }
    println!("\nTotal embeddings completed: {}", COMPLETED_EMBEDDINGS.load(Ordering::SeqCst));
    println!("\nTotal time taken: {} seconds", start_time.elapsed().as_secs_f64());
    PENDING_EMBEDDINGS.store(0, Ordering::SeqCst);
    COMPLETED_EMBEDDINGS.store(0, Ordering::SeqCst);
}

fn clear_screen() {
    clearscreen::clear().expect("Failed to clear screen");
}

async fn get_root_ids(port: u16) -> Result<Vec<String>> {
    let url = format!("http://localhost:{}/{}", port, "getRoot");
    let response = post_request_async(&url, json!({})).await?;
    let root_ids = response
        .get("root")
        .and_then(|v| v.as_array())
        .map(|v| v.iter().map(|v| v.get("id").and_then(|v| v.as_str()).unwrap().to_string()).collect())
        .ok_or_else(|| anyhow::anyhow!("Root ID not found"))?;
    Ok(root_ids)
}

fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    
    rt.block_on(async_main());
}
