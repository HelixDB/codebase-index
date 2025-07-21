mod utils;
mod queries;
mod updater;
mod ingestion;

// External crates
use anyhow::Result;
use rayon::ThreadPoolBuilder;
use serde_json::json;
use std::env;
use std::path::PathBuf;
use std::sync::{
    atomic::Ordering,
    Arc
};
use std::time::Instant;
use std::io;
use std::io::Write;
use tokio::runtime::Runtime;

use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};

// Internal utility functions
use utils::{
    embed_entity_async, post_request_async, EmbeddingJob, post_request,
    ACTIVE_THREADS, TOTAL_CHUNKS, PENDING_EMBEDDINGS, COMPLETED_EMBEDDINGS
};

use updater::update;
use ingestion::ingestion;

/// Main entry point for the codebase ingestion tool
///
/// # Arguments (command line)
/// 1. path: Directory path to index (default: "sample")
/// 2. port: Port number for Helix server (default: 6969)
/// 3. concur_limit: Maximum concurrent operations
fn main() {
    let args: Vec<String> = env::args().collect();

    let default_port = 6969;
    let max_concur = num_cpus::get().saturating_mul(2);
    let update_interval = 10;

    // Get arguments
    let path: String = if args.len() > 1 { args[1].clone() } else { "sample".to_string() };
    let port: u16 = if args.len() > 2 { args[2].parse::<u16>().unwrap() } else { default_port };
    let concur_limit: usize = if args.len() > 3 { args[3].parse::<usize>().unwrap_or(max_concur) } else { max_concur };

    println!("\nConnecting to Helix instance at port {}", port);

    dotenv::dotenv().ok();
    
    // Initialize the global thread pool with the concurrency limit
    ThreadPoolBuilder::new()
        .num_threads(concur_limit)
        .stack_size(8 * 1024 * 1024)
        .build_global()
        .unwrap();
    
    // Create a global Tokio runtime for async operations
    let runtime = Arc::new(Runtime::new().unwrap());

    // Create the Tokio mpsc channel for embedding jobs
    let (tx, rx) = tokio::sync::mpsc::channel::<EmbeddingJob>(concur_limit);

    init_embed_thread(runtime.clone(), rx);

    println!("Initialized thread pool with {} threads", concur_limit);

    let mut root_id = String::new();

    loop {
        root_id = parse_user_input(root_id.clone(), path.clone(), port, runtime.clone(), tx.clone(), update_interval).unwrap();
        if root_id == "EXIT" {
            break;
        }
    }
}


fn parse_user_input(root_id: String, path: String, port: u16, runtime: Arc<Runtime>, tx: tokio::sync::mpsc::Sender<EmbeddingJob>, update_interval: u64) -> Result<String> {
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
            runtime.clone(),
            tx.clone(),
        );

        clear_screen();

        println!("\nTotal chunks processed: {}", TOTAL_CHUNKS.load(Ordering::SeqCst));
        println!("\nIngestion finished in {} seconds", start_time.elapsed().as_secs());

        embedding_wait_thread(update_interval, start_time.clone());

        return Ok(root_id.unwrap().to_string());
    } else if input == "2" {
        clear_screen();
        let root_ids = get_root_ids(port, runtime.clone())?;
        if root_ids.contains(&root_id) {
            println!("\nUpdating index...");
            let _ = update(
                path_buf.canonicalize().unwrap(), root_id.clone(), 
                port, runtime.clone(), tx.clone(), 5
            );
            embedding_wait_thread(update_interval, start_time);
            return Ok(root_id);
        } else {
            println!("\nNo root found");
            return Ok(root_id);
        }
    } else if input == "3" {
        clear_screen();
        return Ok("EXIT".to_string());
    }

    return Err(anyhow::anyhow!("Invalid input"));
}

fn clear_screen() {
    clearscreen::clear().expect("Failed to clear screen");
}

fn get_root_ids(port: u16, runtime: Arc<Runtime>) -> Result<Vec<String>> {
    let url = format!("http://localhost:{}/{}", port, "getRoot");
    let response = post_request(&url, json!({}), &runtime)?;
    let root_ids = response
        .get("root")
        .and_then(|v| v.as_array())
        .map(|v| v.iter().map(|v| v.get("id").and_then(|v| v.as_str()).unwrap().to_string()).collect())
        .ok_or_else(|| anyhow::anyhow!("Root ID not found"))?;
    Ok(root_ids)
}

fn init_embed_thread(
    runtime: Arc<Runtime>,
    rx: tokio::sync::mpsc::Receiver<EmbeddingJob>,
){
    // Spawn the async background task for embedding jobs with concurrent processing
    runtime.spawn(async move {
        // Set concurrent embeddings to better utilize our rate limit
        let max_concurrent_embeddings = 70;
        
        // Create a stream from the channel
        let mut job_stream = tokio_stream::wrappers::ReceiverStream::new(rx)
            .map(|job| {
                async move {
                    while ACTIVE_THREADS.load(Ordering::SeqCst) > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    let EmbeddingJob { chunk, entity_id, port } = job;
                    match embed_entity_async(chunk).await {
                        Ok(embedding) => {
                            let url = format!("http://localhost:{}/{}", port, "embedSuperEntity");
                            let payload = json!({"entity_id": entity_id,"vector": embedding,});
                            if let Err(e) = post_request_async(&url, payload).await {
                                eprintln!("Failed to post embedding: {}", e);
                            }
                        }
                        Err(e) => {
                            eprintln!("Failed to embed chunk: {}", e);
                        }
                    }
                    // Increment completed embeddings counter
                    COMPLETED_EMBEDDINGS.fetch_add(1, Ordering::SeqCst);
                }
            })
            .buffer_unordered(max_concurrent_embeddings);
        
        // Process the stream
        while let Some(_) = job_stream.next().await {}
    });
}

fn embedding_wait_thread(
    update_interval: u64,
    start_time: Instant,
) {
    while ACTIVE_THREADS.load(Ordering::SeqCst) > 0 {
        // Poll the counter every 10ms
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    
    // Wait for all embedding jobs to complete
    println!("Waiting for all embedding jobs to complete...");
    if PENDING_EMBEDDINGS.load(Ordering::SeqCst) > 0 {
        let bar = ProgressBar::new(PENDING_EMBEDDINGS.load(Ordering::SeqCst).try_into().unwrap());
        bar.set_style(
            ProgressStyle::with_template("[{elapsed_precise}] {wide_bar} {pos}/{len} ({per_sec}, ETA: {eta})")
            .unwrap()
        );
        let mut last_completed = 0;
        while COMPLETED_EMBEDDINGS.load(Ordering::SeqCst) < PENDING_EMBEDDINGS.load(Ordering::SeqCst) {
            // Poll the counter every 100ms
            std::thread::sleep(std::time::Duration::from_millis(100));
            
            // Print status every 1 second
            let completed = COMPLETED_EMBEDDINGS.load(Ordering::SeqCst);
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

    TOTAL_CHUNKS.store(0, Ordering::SeqCst);
    ACTIVE_THREADS.store(0, Ordering::SeqCst);
    PENDING_EMBEDDINGS.store(0, Ordering::SeqCst);
    COMPLETED_EMBEDDINGS.store(0, Ordering::SeqCst);

    std::thread::sleep(std::time::Duration::from_secs(update_interval));
}
