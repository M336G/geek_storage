use axum::{Json, extract::{Multipart, State}, http::{HeaderMap, StatusCode}, response::IntoResponse};
use reflink::reflink_or_copy;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::{fs::{self, File}, io::AsyncWriteExt};
use rand::{RngExt, distr::Alphanumeric};

use crate::{AppState, db};

pub fn generate_id() -> String {
    rand::rng()
        .sample_iter(Alphanumeric)
        .take(5)
        .map(char::from)
        .collect()
}

pub async fn upload_file(State(state): State<AppState>, headers: HeaderMap, mut multipart: Multipart) -> impl IntoResponse {
    // If TOKEN is set, check if a Bearer was sent along the request
    if let Some(token) = &state.token {
        let authorization = headers
            .get("Authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));

        match authorization {
            Some(supplied_token) if supplied_token == token => {}
            _ => return (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "error": "You are not allowed to use this!",
                    "id": null
                }))
            ),
        }
    }

    if let Err(error) = fs::create_dir_all(&state.temporary_path).await {
        eprintln!("Failed to create temporary directory: {error}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": "Failed uploading your file",
                "id": null
            }))
        );
    }
    
    let mut id = generate_id();
    while db::file_exists(&state.connection, &id).await {
        id = generate_id();
    }

    let temp_path = state.temporary_path.join(&id);

    let storage_left = state.max_storage_limit.saturating_sub(db::get_total_size(&state.connection).await);
    let mut size: u64 = 0;
    let mut saved = false;
    let mut hasher = Sha256::new();

    // Go through every field in the multipart form
    while let Some(mut field) = multipart.next_field().await.unwrap() {
        match field.name().unwrap_or("") {
            "file" => {
                let mut file = match File::create(&temp_path).await {
                    Ok(f) => f,
                    Err(error) => {
                        eprintln!("Failed to create temporary file: {error}");
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({
                                "error": "Failed uploading your file",
                                "id": null
                            }))
                        );
                    }
                };

                while let Some(chunk) = field.chunk().await.unwrap() {
                    size += chunk.len() as u64;

                    // Make sure it doesn't exceed the maximum file size limit
                    if size > state.max_file_size {
                        drop(file);
                        let _ = fs::remove_file(&temp_path).await;
                        return (
                            StatusCode::PAYLOAD_TOO_LARGE,
                            Json(json!({
                                "error": format!("File exceeds the maximum file size limit of {} megabytes", state.max_file_size / 1024 / 1024),
                                "id": null
                            }))
                        );
                    }

                    // And that there is also enough storage left
                    if size > storage_left {
                        drop(file);
                        let _ = fs::remove_file(&temp_path).await;
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({
                                "error": "The server is completely full!",
                                "id": null
                            }))
                        );
                    }

                    hasher.update(&chunk);

                    if let Err(error) = file.write_all(&chunk).await {
                        eprintln!("Failed to write chunk: {error}");
                        drop(file);
                        let _ = fs::remove_file(&temp_path).await;
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({
                                "error": "Failed uploading your file",
                                "id": null
                            }))
                        );
                    }
                }

                saved = true;
                break;
            }
            "link" => {
                let link = field.text().await.unwrap();

                let mut response = match state.client.get(link).send().await {
                    Ok(response) => response,
                    Err(error) => {
                        eprintln!("Failed link download: {error}");
                        return (
                            StatusCode::BAD_GATEWAY,
                            Json(json!({
                                "error": "Failed downloading from the link supplied",
                                "id": null
                            }))
                        );
                    }
                };

                let mut file = match File::create(&temp_path).await {
                    Ok(f) => f,
                    Err(error) => {
                        eprintln!("Failed to create temporary file: {error}");
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({
                                "error": "Failed uploading your file",
                                "id": null
                            }))
                        );
                    }
                };

                while let Some(chunk) = response.chunk().await.unwrap() {
                    size += chunk.len() as u64;

                    // Make sure it doesn't exceed the maximum file size limit
                    if size > state.max_file_size {
                        drop(file);
                        let _ = fs::remove_file(&temp_path).await;
                        return (
                            StatusCode::PAYLOAD_TOO_LARGE,
                            Json(json!({
                                "error": format!("File exceeds the maximum file size limit of {} megabytes", state.max_file_size / 1024 / 1024),
                                "id": null
                            }))
                        );
                    }

                    // And that there is also enough storage left
                    if size > storage_left {
                        drop(file);
                        let _ = fs::remove_file(&temp_path).await;
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({
                                "error": "The server is completely full!",
                                "id": null
                            }))
                        );
                    }

                    hasher.update(&chunk);

                    if let Err(error) = file.write_all(&chunk).await {
                        eprintln!("Failed to write chunk: {error}");
                        drop(file);
                        let _ = fs::remove_file(&temp_path).await;
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({
                                "error": "Failed uploading your file",
                                "id": null
                            }))
                        );
                    }
                }

                saved = true;
                break;
            }
            _ => {}
        }
    }

    if !saved {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "Please send either a file or a link",
                "id": null
            }))
        );
    }

    let hash = hex::encode(hasher.finalize());

    // If the file already exists no need to recreate it
    if let Some(existing_id) = db::hash_exists(&state.connection, &hash).await {
        let _ = fs::remove_file(&temp_path).await;
        return(
            StatusCode::OK,
            Json(json!({
                "error": null,
                "id": existing_id
            }))
        )
    }

    // First try writing the file to its actual destination
    let final_path = state.storage_path.join(&id);
    match fs::rename(&temp_path, &final_path).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            if let Err(error) = reflink_or_copy(&temp_path, &final_path) {
                eprintln!("Failed to copy file: {error}");
                let _ = fs::remove_file(&temp_path).await;
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": "Failed uploading your file",
                        "id": null
                    }))
                );
            }

            let _ = fs::remove_file(&temp_path).await;
        }
        Err(error) => {
            eprintln!("Failed to move file: {error}");
            let _ = fs::remove_file(&temp_path).await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Failed uploading your file",
                    "id": null
                }))
            );
        }
    }

    // And try to add it to the database too
    if let Err(error) = db::add_file(&state.connection, &db::File {
        id: id.clone(),
        hash,
        size
    }).await {
        eprintln!("Failed to add file to database: {error}");
        let _ = fs::remove_file(&final_path).await;
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": "Failed uploading your file",
                "id": null
            }))
        );
    }

    println!("New file: {id}");
    (
        StatusCode::OK,
        Json(json!({
            "error": null,
            "id": id
        }))
    )
}