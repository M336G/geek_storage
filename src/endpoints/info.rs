use axum::{Json, extract::State};
use serde_json::json;

use crate::{AppState, db};

pub async fn get_server_info(State(state): State<AppState>) -> Json<serde_json::Value> {
    let total_files = db::get_total_files(&state.connection).await;
    let total_size = db::get_total_size(&state.connection).await;

    // All of this is just to make sure the application doesn't
    // crash if total_files or total_size are 0
    let estimated_max_files = if total_files > 0 && total_size > 0 {
        Some(state.max_storage_limit / (total_size / total_files))
    } else {
        None
    };

    Json(json!({
        "error": null,
        "version": env!("CARGO_PKG_VERSION"),
        "files": {
            "max": estimated_max_files,
            "total": total_files
        },
        "size": {
            "max": state.max_storage_limit,
            "total": total_size
        }
    }))
}
