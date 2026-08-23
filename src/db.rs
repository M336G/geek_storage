use chrono::Utc;
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};

pub struct File {
    pub id: String,
    pub hash: String,
    pub size: u64
}

// Initialize the database
pub async fn open() -> SqlitePool {
    std::fs::create_dir_all("data").unwrap();

    let options = SqliteConnectOptions::new()
        .filename("data/database.db")
        .create_if_missing(true);

    let pool = SqlitePool::connect_with(options)
        .await
        .unwrap();

    // This should make it much faster overall
    sqlx::query("PRAGMA journal_mode = WAL;")
        .execute(&pool)
        .await
        .unwrap();

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("Failed running SQL migrations");

    pool
}

// Check if a file with the provided ID exists
pub async fn file_exists(pool: &SqlitePool, id: &String) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query("SELECT 1 FROM files WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .is_some())
}

// Check if a file with the provided hash exists
pub async fn hash_exists(pool: &SqlitePool, hash: &String) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM files WHERE hash = ?")
        .bind(hash)
        .fetch_optional(pool)
        .await
}

// Get the total amount of files in the database
pub async fn get_total_files(pool: &SqlitePool) -> u64 {
    sqlx::query_scalar("SELECT COUNT(id) FROM files")
        .fetch_one(pool)
        .await
        .unwrap_or(None)
        .unwrap_or(0) as u64
}

// Get the total size of all combined files in the database
pub async fn get_total_size(pool: &SqlitePool) -> u64 {
    sqlx::query_scalar("SELECT SUM(size) FROM files")
        .fetch_one(pool)
        .await
        .unwrap_or(None)
        .unwrap_or(0) as u64
}

// Add a new file to the database
pub async fn add_file(pool: &SqlitePool, file: &File) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("INSERT OR IGNORE INTO files (id, hash, size) VALUES (?, ?, ?)")
        .bind(&file.id)
        .bind(&file.hash)
        .bind(file.size as i64)
        .execute(pool)
        .await?;

    Ok(result.rows_affected() > 0)
}

// Delete an existing file from the database
pub async fn delete_file(pool: &SqlitePool, id: &String) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM files WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// Get the ID of every file in the database
pub async fn get_all_files(pool: &SqlitePool) -> Vec<String> {
    sqlx::query_scalar("SELECT id FROM files")
        .fetch_all(pool)
        .await
        .unwrap_or_default()
}

// Update the time when a file was last accessed
pub async fn update_last_accessed(pool: &SqlitePool, id: &String) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE files SET lastAccessedOn = ? WHERE id = ?")
        .bind(Utc::now().timestamp())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// Get the ID of every expired file
pub async fn get_expired_files(pool: &SqlitePool, max_unaccessed_time: u64) -> Vec<String> {
    sqlx::query_scalar("SELECT id FROM files WHERE lastAccessedOn IS NULL OR lastAccessedOn < ?")
        .bind(Utc::now().timestamp() - max_unaccessed_time as i64)
        .fetch_all(pool)
        .await
        .unwrap_or_default()
}