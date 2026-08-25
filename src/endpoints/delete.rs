use axum::{Json, extract::{Path, State}, http::{HeaderMap, StatusCode}, response::IntoResponse};
use serde_json::json;
use tokio::fs;

use crate::{AppState, db};

pub async fn delete_file(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> impl IntoResponse {
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
                Json(json!({ "error": "You are not allowed to use this!" }))
            ),
        }
    }

    // Check if the ID is 5 characters and is alphanumeric
    if id.len() != 5 || !id.chars().all(|character| character.is_ascii_alphanumeric()) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "This file does not exist!" }))
        );
    }

    // Make sure the file even exists
    let exists = match db::file_exists(&state.connection, &id).await {
        Ok(exists) => exists,
        Err(error) => {
            eprintln!("Failed to check if file existed: {error}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed checking for file" }))
            );
        }
    };
    if !exists {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "This file does not exist!" }))
        );
    }

    // Delete the file from the database
    if let Err(error) = db::delete_file(&state.connection, &id).await {
        eprintln!("Failed to delete file from database: {error}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "Failed deleting file" }))
        );
    }

    // Delete the file from the storage path
    if let Err(error) = fs::remove_file(state.storage_path.join(&id)).await {
        eprintln!("Failed to delete file from storage: {error}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "Failed deleting file" }))
        );
    }

    (StatusCode::OK, Json(json!({ "error": null })))
}