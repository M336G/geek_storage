use axum::{extract::{Path, State}, http::{StatusCode, header}, response::{IntoResponse, Response}, body::Body};
use tokio::fs::File;
use tokio_util::io::ReaderStream;

use crate::{AppState, db};

pub async fn get_file(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    // Make sure the ID is 5 characters, is alphanumeric and that it exists
    if id.len() != 5 || !id.chars().all(|character| character.is_ascii_alphanumeric()) || !db::file_exists(&state.connection, &id).await {
        return (StatusCode::NOT_FOUND, "This file does not exist!").into_response();
    }

    let file_path = state.storage_path.join(&id);

    let file = match File::open(&file_path).await {
        Ok(file) => file,
        Err(error) => {
            eprintln!("Failed to open file: {error}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed reading file").into_response();
        }
    };

    if let Err(error) = db::update_last_accessed(&state.connection, &id).await {
        eprintln!("Could not update {id}'s last access: {error}");
    }

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "public, max-age=86400")], // Cached for 24h
        body
    ).into_response()
}