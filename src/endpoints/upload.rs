use axum::{Json, extract::{FromRequest, Multipart, Request, State}, http::{HeaderMap, StatusCode}, response::IntoResponse};
use reflink::reflink_or_copy;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::future::Future;
use tokio::{fs::{self, File}, io::AsyncWriteExt, task::spawn_blocking};
use rand::{RngExt, distr::Alphanumeric};

use crate::{AppState, db};

pub struct MultipartForm(pub Multipart);

impl<S: Send + Sync> FromRequest<S> for MultipartForm {
    type Rejection = (StatusCode, Json<serde_json::Value>);

    fn from_request(req: Request, state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            Multipart::from_request(req, state)
                .await
                .map(MultipartForm)
                .map_err(|_| (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": "Invalid/missing multipart form data",
                        "id": null
                    }))
                ))
        }
    }
}

pub fn generate_id() -> String {
    rand::rng()
        .sample_iter(Alphanumeric)
        .take(5)
        .map(char::from)
        .collect()
}

pub async fn upload_file(State(state): State<AppState>, headers: HeaderMap, MultipartForm(mut multipart): MultipartForm,) -> impl IntoResponse {
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
    loop {
        match db::file_exists(&state.connection, &id).await {
            Ok(false) => break,
            Ok(true) => id = generate_id(),
            Err(error) => {
                eprintln!("Failed to check if id existed: {error}");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": "Failed uploading your file",
                        "id": null
                    }))
                );
            }
        }
    }

    let temp_path = state.temporary_path.join(&id);

    let mut size: u64 = 0;
    let mut saved = false;
    let mut hasher = Sha256::new();

    let storage_left = if state.max_storage_limit == 0 {
        None
    } else {
        Some(state.max_storage_limit.saturating_sub(db::get_total_size(&state.connection).await))
    };

    // Go through every field in the multipart form
    loop {
        let mut field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(_) => return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "Invalid or missing multipart form data",
                    "id": null
                }))
            ),
        };

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

                loop {
                    let chunk = match field.chunk().await {
                        Ok(Some(chunk)) => chunk,
                        Ok(None) => break,
                        Err(error) => {
                            eprintln!("Failed to read chunk: {error}");
                            drop(file);
                            let _ = fs::remove_file(&temp_path).await;
                            return (
                                StatusCode::BAD_REQUEST,
                                Json(json!({
                                    "error": "Failed reading uploaded file data",
                                    "id": null
                                }))
                            );
                        }
                    };

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
                    if storage_left.is_some_and(|left| size > left) {
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
                let link = match field.text().await {
                    Ok(text) => text,
                    Err(error) => {
                        eprintln!("Failed to read link field: {error}");
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "error": "Failed reading the link field",
                                "id": null
                            }))
                        );
                    }
                };

                let mut response = match state.client.get(link).send().await {
                    Ok(response) => match response.error_for_status() {
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
                    },
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

                loop {
                    let chunk = match response.chunk().await {
                        Ok(Some(chunk)) => chunk,
                        Ok(None) => break,
                        Err(error) => {
                            eprintln!("Failed to read chunk from link: {error}");
                            drop(file);
                            let _ = fs::remove_file(&temp_path).await;
                            return (
                                StatusCode::BAD_GATEWAY,
                                Json(json!({
                                    "error": "Failed downloading from the link supplied",
                                    "id": null
                                }))
                            );
                        }
                    };

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
                    if storage_left.is_some_and(|left| size > left) {
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

    // Make sure there is enough storage left
    if state.max_storage_limit > 0 {
        let current_total = db::get_total_size(&state.connection).await;
        if current_total.saturating_add(size) > state.max_storage_limit {
            let _ = fs::remove_file(&temp_path).await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "The server is completely full!",
                    "id": null
                }))
            );
        }
    }

    // If the file already exists no need to recreate it
    match db::hash_exists(&state.connection, &hash).await {
        Ok(Some(existing_id)) => {
            let _ = fs::remove_file(&temp_path).await;
            return (
                StatusCode::OK,
                Json(json!({
                    "error": null,
                    "id": existing_id
                }))
            );
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("Failed to check for existing hash: {error}");
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

    // First try writing the file to its actual destination
    let final_path = state.storage_path.join(&id);
    match fs::rename(&temp_path, &final_path).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            let temp_path_copy = temp_path.clone();
            let final_path_copy = final_path.clone();

            match spawn_blocking(move || reflink_or_copy(&temp_path_copy, &final_path_copy)).await {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
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
                Err(error) => {
                    eprintln!("Failed copying file in blocking task: {error}");
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

    // Check if a file with the same hash doesn't already exist after all of that
    match db::hash_exists(&state.connection, &hash).await {
        Ok(Some(existing_id)) => {
            let _ = fs::remove_file(&final_path).await;
            return (
                StatusCode::OK,
                Json(json!({
                    "error": null,
                    "id": existing_id
                }))
            );
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("Failed to check for existing hash: {error}");
            let _ = fs::remove_file(&final_path).await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Failed uploading your file",
                    "id": null
                }))
            );
        }
    }

    match db::add_file(&state.connection, &db::File { id: id.clone(), hash: hash.clone(), size }).await {
        Ok(true) => {}
        Ok(false) => {
            let _ = fs::remove_file(&final_path).await;
            let original_id = db::hash_exists(&state.connection, &hash).await
                .ok().flatten().unwrap_or(id);

            return (
                StatusCode::OK,
                Json(json!({
                    "error": null,
                    "id": original_id
                }))
            );
        }
        Err(error) => {
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