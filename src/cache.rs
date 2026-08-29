use bytes::Bytes;
use dashmap::DashMap;
use std::{sync::{atomic::{AtomicUsize, Ordering}, Arc, Mutex}, time::Instant};

pub struct CacheEntry {
    pub data: Arc<Bytes>,
    pub size_bytes: usize,
    pub last_accessed: Instant,
}

pub struct FileCache {
    entries: DashMap<String, CacheEntry>,
    max_size_bytes: usize,
    current_size_bytes: AtomicUsize,
    write_lock: Mutex<()>
}

impl FileCache {
    // Make a new instance of the FileCache
    pub fn new(max_size_mb: u16) -> Self {
        Self {
            entries: DashMap::new(),
            max_size_bytes: max_size_mb as usize * 1024 * 1024,
            current_size_bytes: AtomicUsize::new(0),
            write_lock: Mutex::new(()),
        }
    }

    // Tells the maximum size an entry can be (no more than 20% of the maximum cache size)
    pub fn get_max_size_per_entry(&self) -> usize {
        (self.max_size_bytes as f64 * 0.2) as usize
    }

    // Get an entry from the FileCache
    pub fn get(&self, id: &str) -> Option<Arc<Bytes>> {
        let mut entry = self.entries.get_mut(id)?;
        entry.last_accessed = Instant::now();
        Some(Arc::clone(&entry.data))
    }

    // Add an entry to the FileCache
    pub fn insert(&self, id: String, data: Vec<u8>) -> bool {
        let entry_size = data.len();

        // If it's more than 20% of the max cache limit don't continue
        if entry_size > self.get_max_size_per_entry() {
            return false;
        }

        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());

        // If it already exists don't continue
        if self.entries.contains_key(&id) {
            return false;
        }

        // Remove the oldest entries until there is enough space for the new one
        while self.current_size_bytes.load(Ordering::Relaxed) + entry_size > self.max_size_bytes {
            // Find the least recently accessed entry
            let oldest_key = self
                .entries
                .iter()
                .min_by_key(|entry| entry.last_accessed)
                .map(|entry| entry.key().clone());

            match oldest_key {
                Some(key) => {
                    if let Some((_, entry)) = self.entries.remove(&key) {
                        self.current_size_bytes
                            .fetch_sub(entry.size_bytes, Ordering::Relaxed);
                    }
                }
                None => break, // If the cache is empty but the file still cannot be added (somehow) just stop there
            }
        }

        // Add the entry and update the cache size
        self.current_size_bytes.fetch_add(entry_size, Ordering::Relaxed);
        self.entries.insert(
            id,
            CacheEntry {
                data: Arc::new(Bytes::from(data)),
                size_bytes: entry_size,
                last_accessed: Instant::now(),
            },
        );

        true
    }

    // Remove an entry from the FileCache
    pub fn remove(&self, id: &str) {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        
        if let Some((_, entry)) = self.entries.remove(id) {
            self.current_size_bytes
                .fetch_sub(entry.size_bytes, Ordering::Relaxed);
        }
    }
}