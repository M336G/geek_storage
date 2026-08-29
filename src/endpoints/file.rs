use axum::{extract::{Path, State}, http::{StatusCode, header}, response::{IntoResponse, Response}, body::Body};
use tokio::{fs::File, io::AsyncReadExt};
use tokio_util::io::ReaderStream;

use crate::{AppState, db};

const CACHE_CONTROL_HEADER: (header::HeaderName, &str) =
    (header::CACHE_CONTROL, "public, max-age=86400"); // Cache for 24h

pub async fn get_file(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    // Make sure the ID is 5 characters and is alphanumeric
    if id.len() != 5 || !id.chars().all(|character| character.is_ascii_alphanumeric()) {
        return (StatusCode::NOT_FOUND, "This file does not exist!").into_response();
    }

    let cache = state.cache.as_ref();

    // If the file is cached, return it directly
    if let Some(data) = cache.and_then(|cache| cache.get(&id)) {
        if let Err(error) = db::update_last_accessed(&state.connection, &id).await {
            eprintln!("Could not update {id}'s last access: {error}");
        }
        return (StatusCode::OK, [CACHE_CONTROL_HEADER], Body::from((*data).clone())).into_response();
    }


    // Make sure the file even exists
    let exists = match db::file_exists(&state.connection, &id).await {
        Ok(exists) => exists,
        Err(error) => {
            eprintln!("Failed to check if file existed: {error}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed checking for file").into_response();
        }
    };
    if !exists {
        return (StatusCode::NOT_FOUND, "This file does not exist!").into_response();
    }

    let file_path = state.storage_path.join(&id);

    let mut file = match File::open(&file_path).await {
        Ok(file) => file,
        Err(error) => {
            eprintln!("Failed to open file: {error}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed reading file").into_response();
        }
    };

    if let Err(error) = db::update_last_accessed(&state.connection, &id).await {
        eprintln!("Could not update {id}'s last access: {error}");
    }
    
    let file_size = file.metadata().await.map(|metadata| metadata.len() as usize).unwrap_or(0);
    let cacheable = cache
        .map(|cache| file_size > 0 && file_size <= cache.get_max_size_per_entry())
        .unwrap_or(false);
    
    // Return the file after caching it if it's possible
    if cacheable {
        let cache = cache.unwrap();

        let mut buffer = Vec::with_capacity(file_size);
        match file.read_to_end(&mut buffer).await {
            Ok(_) => {
                cache.insert(id.clone(), buffer.clone());
                return (StatusCode::OK, [CACHE_CONTROL_HEADER], Body::from(buffer)).into_response()
            }
            Err(error) => {
                eprintln!("Failed caching {id} into memory: {error}");

                // Re-open the file for the default fallback below
                file = match File::open(&file_path).await {
                    Ok(file) => file,
                    Err(error) => {
                        eprintln!("Failed to reopen file: {error}");
                        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed reading file").into_response();
                    }
                };
            }
        }
    }

    // Otherwise just return a normal file stream
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    return (StatusCode::OK, [CACHE_CONTROL_HEADER], body).into_response()
}