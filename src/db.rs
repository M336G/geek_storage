use std::sync::{OnceLock, atomic::{AtomicU64, Ordering}};
use chrono::Utc;
use dashmap::DashMap;
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};

pub struct File {
    pub id: String,
    pub hash: String,
    pub size: u64
}
struct CachedFile {
    hash: String,
    size: u64,
    last_accessed: Option<i64>
}

static FILES: OnceLock<DashMap<String, CachedFile>> = OnceLock::new();
static HASHES: OnceLock<DashMap<String, String>> = OnceLock::new();

static TOTAL_FILES: AtomicU64 = AtomicU64::new(0);
static TOTAL_SIZE: AtomicU64 = AtomicU64::new(0);

fn files() -> &'static DashMap<String, CachedFile> {
    FILES.get_or_init(DashMap::new)
}

fn hashes() -> &'static DashMap<String, String> {
    HASHES.get_or_init(DashMap::new)
}

// Initialize the database
pub async fn open() -> SqlitePool {
    std::fs::create_dir_all("data").expect("Failed creating data directory");

    let options = SqliteConnectOptions::new()
        .filename("data/database.db")
        .create_if_missing(true);

    let pool = SqlitePool::connect_with(options)
        .await
        .expect("Failed connecting to database");

    // This should make it much faster overall
    sqlx::query("PRAGMA journal_mode = WAL;")
        .execute(&pool)
        .await
        .expect("Failed enabling WAL mode");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("Failed running SQL migrations");

    let rows: Vec<(String, String, i64, Option<i64>)> =
        sqlx::query_as("SELECT id, hash, size, lastAccessedOn FROM files")
            .fetch_all(&pool)
            .await
            .expect("Failed loading files into cache");

    let mut total_size = 0u64;

    for (id, hash, size, last_accessed) in rows {
        total_size += size as u64;
        hashes().insert(hash.clone(), id.clone());
        files().insert(id, CachedFile { hash, size: size as u64, last_accessed });
    }

    TOTAL_FILES.store(files().len() as u64, Ordering::Relaxed);
    TOTAL_SIZE.store(total_size, Ordering::Relaxed);

    pool
}

// Check if a file with the provided ID exists
pub async fn file_exists(_pool: &SqlitePool, id: &String) -> Result<bool, sqlx::Error> {
    Ok(files().contains_key(id))
}

// Check if a file with the provided hash exists
pub async fn hash_exists(_pool: &SqlitePool, hash: &String) -> Result<Option<String>, sqlx::Error> {
    Ok(hashes().get(hash).map(|id| id.value().clone()))
}

// Get the total amount of files in the database
pub async fn get_total_files(_pool: &SqlitePool) -> u64 {
    TOTAL_FILES.load(Ordering::Relaxed)
}

// Get the total size of all combined files in the database
pub async fn get_total_size(_pool: &SqlitePool) -> u64 {
    TOTAL_SIZE.load(Ordering::Relaxed)
}

// Add a new file to the database
pub async fn add_file(pool: &SqlitePool, file: &File) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("INSERT OR IGNORE INTO files (id, hash, size) VALUES (?, ?, ?)")
        .bind(&file.id)
        .bind(&file.hash)
        .bind(file.size as i64)
        .execute(pool)
        .await?;

    let inserted = result.rows_affected() > 0;

    if inserted {
        hashes().insert(file.hash.clone(), file.id.clone());
        files().insert(file.id.clone(), CachedFile {
            hash: file.hash.clone(),
            size: file.size,
            last_accessed: None
        });
        TOTAL_FILES.fetch_add(1, Ordering::Relaxed);
        TOTAL_SIZE.fetch_add(file.size, Ordering::Relaxed);
    }

    Ok(inserted)
}

// Delete an existing file from the database
pub async fn delete_file(pool: &SqlitePool, id: &String) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM files WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    if let Some((_, cached)) = files().remove(id) {
        hashes().remove_if(&cached.hash, |_, owner| owner == id);
        TOTAL_FILES.fetch_sub(1, Ordering::Relaxed);
        TOTAL_SIZE.fetch_sub(cached.size, Ordering::Relaxed);
    }

    Ok(())
}

// Get the ID of every file in the database
pub async fn get_all_files(_pool: &SqlitePool) -> Vec<String> {
    files().iter().map(|entry| entry.key().clone()).collect()
}

// Update the time when a file was last accessed
pub async fn update_last_accessed(pool: &SqlitePool, id: &String) -> Result<(), sqlx::Error> {
    let now = Utc::now().timestamp();

    if let Some(mut entry) = files().get_mut(id) {
        entry.last_accessed = Some(now);
    }

    sqlx::query("UPDATE files SET lastAccessedOn = ? WHERE id = ?")
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;

    Ok(())
}

// Get the ID of every expired file
pub async fn get_expired_files(_pool: &SqlitePool, max_unaccessed_time: u64) -> Vec<String> {
    let threshold = Utc::now().timestamp() - max_unaccessed_time as i64;

    files()
        .iter()
        .filter(|entry| entry.last_accessed.map_or(true, |timestamp| timestamp < threshold))
        .map(|entry| entry.key().clone())
        .collect()
}