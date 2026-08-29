use axum::{Router, extract::DefaultBodyLimit, routing::{get, post, delete}};
use dotenv::dotenv;
use reqwest::Client;
use sqlx::SqlitePool;
use tokio::fs;
use tower_http::compression::CompressionLayer;
use axum::http::Method;
use tower_http::cors::{self, CorsLayer};
use std::{env::temp_dir, sync::Arc};
use std::process;
use std::path::PathBuf;
use std::{env, time::Duration};
use tokio::time::sleep;
use tokio::net::TcpListener;

mod db;
mod endpoints;
mod cache;

use cache::FileCache;

// Stores some "global variables" which will be used across the whole program
#[derive(Clone)]
struct AppState {
    connection: SqlitePool,
    client: Client,
    cache: Option<Arc<FileCache>>,
    token: Option<String>,
    storage_path: PathBuf,
    temporary_path: PathBuf,
    max_storage_limit: u64,
    max_file_size: u64,
    max_unaccessed_time: u64
}

#[tokio::main]
async fn main() {
    dotenv().ok(); // Load from .env

    // Initialize all needed variables here
    let server_port: u16 = env::var("PORT")
        .ok()
        .and_then(|port| port.parse().ok())
        .unwrap_or(8912);

    let token = env::var("TOKEN").ok();

    let storage_path: PathBuf = match env::var("STORAGE_PATH") {
        Ok(path) => path.into(),
        Err(_) => {
            eprintln!("STORAGE_PATH environment variable must be set!");
            process::exit(1);
        }
    };

    if !storage_path.is_dir() {
        eprintln!("STORAGE_PATH is not a directory or does not exist!");
        process::exit(1);
    }

    let max_storage_limit = env::var("MAX_STORAGE_LIMIT")
        .ok()
        .and_then(|limit| limit.parse().ok())
        .map(|gb: u64| gb * 1024 * 1024 * 1024)
        .unwrap_or(0);

    let max_file_size = env::var("MAX_FILE_SIZE")
        .ok()
        .and_then(|size| size.parse().ok())
        .map(|mb: u64| mb * 1024 * 1024)
        .unwrap_or(10 * 1024 * 1024);

    let max_unaccessed_time = env::var("MAX_UNACCESSED_TIME")
        .ok()
        .and_then(|hours| hours.parse().ok())
        .map(|hours: u64| hours * 60 * 60)
        .unwrap_or(0);

    let max_cache_size: Option<u16> = env::var("MAX_CACHE_SIZE")
        .ok()
        .and_then(|size| size.parse().ok());
    if let Some(size) = &max_cache_size {
        println!("Max file cache size set to {size} MB!");
    } else {
        println!("File cache disabled!");
    }

    let client = Client::builder()
        .user_agent(format!("GeekStorage/{}", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    let cache = max_cache_size.map(|mb| Arc::new(FileCache::new(mb)));

    let state = AppState {
        connection: db::open().await,
        client,
        cache,
        token,
        storage_path,
        temporary_path: temp_dir().join("geek_storage"),
        max_storage_limit,
        max_file_size,
        max_unaccessed_time
    };

    if max_unaccessed_time > 0 {
        // Check for files that have expired
        let expired_check_state = state.clone();
        tokio::spawn(async move {
            loop {
                let expired_files = db::get_expired_files(&expired_check_state.connection, expired_check_state.max_unaccessed_time).await;
                let mut success = 0;
                let mut fail = 0;

                for expired_file in &expired_files {
                    if let Err(error) = db::delete_file(&expired_check_state.connection, &expired_file).await {
                        eprintln!("Failed cleaning up expired file: {error}");
                        fail += 1;
                        continue;
                    }

                    if let Err(error) = fs::remove_file(expired_check_state.storage_path.join(&expired_file)).await {
                        eprintln!("Failed cleaning up expired file: {error}");
                        fail += 1;
                        continue;
                    }

                    if let Some(cache) = expired_check_state.cache.as_ref() {
                        cache.remove(expired_file);
                    }

                    success += 1;
                }

                if !expired_files.is_empty() {
                    println!("Cleaned up {}/{} expired files ({} fails)", success, expired_files.len(), fail);
                }

                sleep(Duration::from_mins(1)).await;
            }
        });
    }

    // Check for files that exist in database but not in the storage
    let invalid_check_state = state.clone();
    tokio::spawn(async move {
        loop {
            let ids = db::get_all_files(&invalid_check_state.connection).await;
            let mut success = 0;
            let mut fail = 0;

            for id in ids {
                if !invalid_check_state.storage_path.join(&id).exists() {
                    if let Err(error) = db::delete_file(&invalid_check_state.connection, &id).await {
                        eprintln!("Failed cleaning up invalid file: {error}");
                        fail += 1;
                        continue;
                    }

                    success += 1;
                }
            }

            let total = success + fail;
            if total > 0 {
                println!("Cleaned up {}/{} invalid files ({} fails)", success, total, fail);
            }

            sleep(Duration::from_mins(30)).await;
        }
    });

    // Check for files that exist in storage but not in the database
    let unknown_check_state = state.clone();
    tokio::spawn(async move {
        loop {
            if let Ok(mut directory) = fs::read_dir(&unknown_check_state.storage_path).await {
                let mut success = 0;
                let mut fail = 0;

                while let Ok(Some(entry)) = directory.next_entry().await {
                    let id = entry.file_name().to_string_lossy().to_string();
                        
                    let exists = match db::file_exists(&unknown_check_state.connection, &id).await {
                        Ok(exists) => exists,
                        Err(error) => {
                            eprintln!("Failed cleaning up unknown file (check error): {error}");
                            fail += 1;
                            continue;
                        }
                    };
                    if !exists {
                        if let Err(error) = fs::remove_file(unknown_check_state.storage_path.join(id)).await {
                            eprintln!("Failed cleaning up unknown file (delete error): {error}");
                            fail += 1;
                            continue;
                        }

                        success += 1;
                    }
                }

                let total = success + fail;
                if total > 0 {
                    println!("Cleaned up {}/{} unknown files ({} fails)", success, total, fail);
                }
            }

            sleep(Duration::from_mins(30)).await;
        }
    });

    let app = Router::new()
        .route("/", get(endpoints::health_check))
        .route("/", post(endpoints::upload_file))
        .route("/{id}", delete(endpoints::delete_file))
        .route("/{id}", get(endpoints::get_file))
        .route("/info", get(endpoints::get_server_info))
        .layer(DefaultBodyLimit::disable())
        .layer(CompressionLayer::new())
        .layer(CorsLayer::new()
            .allow_origin(cors::Any)
            .allow_methods([Method::GET, Method::POST])
        )
        .with_state(state);

    println!("Server running on http://0.0.0.0:{server_port}/");
    axum::serve(
        TcpListener::bind(format!("0.0.0.0:{server_port}"))
            .await
            .unwrap(),
        app,
    )
    .await
    .unwrap();
}