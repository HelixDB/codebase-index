use anyhow::Result;
use std::collections::HashMap;
use serde_json::json;

use crate::utils::{post_request_async};

pub async fn get_root_folders (
    root_id: String,
    port: u16
) -> Result<HashMap<String, String>> {
    let url = format!("http://localhost:{}/{}", port, "getRootFolders");
    let root_folder_res = post_request_async(&url, json!({ "root_id": root_id })).await?;
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

    Ok(root_folder_name_ids)
}

pub async fn get_root_files (
    root_id: String,
    port: u16
) -> Result<HashMap<String, (String, String)>> {
    let url = format!("http://localhost:{}/{}", port, "getRootFiles");
    let payload = json!({ "root_id": root_id });
    let root_file_res = post_request_async(&url, payload).await?;
    let root_files = root_file_res
        .get("files")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("Root ID not found"))?;

    let mut root_file_name_ids: HashMap<String, (String, String)> = HashMap::new();

    for file in root_files {
        let file_id = file.get("id").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("File ID not found"))?;
        let file_name = file.get("name").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("File name not found"))?;
        let file_extracted_at = file.get("extracted_at").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("File extracted at not found"))?;
        root_file_name_ids.insert(file_name.to_string(), (file_id.to_string(), file_extracted_at.to_string()));
    }

    Ok(root_file_name_ids)
}

pub async fn get_sub_folders (
    folder_id: String,
    port: u16
) -> Result<HashMap<String, String>> {
    let url = format!("http://localhost:{}/{}", port, "getSubFolders");
    let payload = json!({ "folder_id": folder_id });
    let folder_res = post_request_async(&url, payload).await?;
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

    Ok(subfolder_name_ids)    
}

pub async fn get_folder_files (
    folder_id: String,
    port: u16
) -> Result<HashMap<String, (String, String)>> {
    let url = format!("http://localhost:{}/{}", port, "getFolderFiles");
    let payload = json!({ "folder_id": folder_id });
    let folder_file_res = post_request_async(&url, payload).await?;
    let folder_files = folder_file_res
        .get("files")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("Folder ID not found"))?;

    let mut folder_file_name_ids: HashMap<String, (String, String)> = HashMap::new();

    for file in folder_files {
        let file_id = file.get("id").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("File ID not found"))?;
        let file_name = file.get("name").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("File name not found"))?;
        let file_extracted_at = file.get("extracted_at").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("File extracted at not found"))?;
        folder_file_name_ids.insert(file_name.to_string(), (file_id.to_string(), file_extracted_at.to_string()));
    }

    Ok(folder_file_name_ids)
}