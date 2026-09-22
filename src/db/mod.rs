pub mod integrations;

use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;

pub use integrations::{Repo, RepoError};

pub async fn connect(url: &str) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::from_str(url)?
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    strip_retired_fields(&pool).await?;
    Ok(pool)
}

/// Keys that documents may still carry and the spec no longer accepts. A
/// stored document is JSON that SQL cannot walk, so the sweep lives here
/// rather than in a migration; it is a no-op once every row is clean.
const RETIRED: [&str; 9] = [
    "description",
    "enabled",
    "query",
    "timeout_ms",
    "success",
    "code",
    "ty",
    "required",
    "default",
];

async fn strip_retired_fields(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let rows = sqlx::query("select id, spec from integrations")
        .fetch_all(pool)
        .await?;

    for row in rows {
        let spec: String = row.get("spec");
        let Ok(mut doc) = serde_json::from_str::<Value>(&spec) else {
            continue;
        };
        if !strip(&mut doc) {
            continue;
        }
        let Ok(cleaned) = serde_json::to_string_pretty(&doc) else {
            continue;
        };
        sqlx::query("update integrations set spec = ? where id = ?")
            .bind(&cleaned)
            .bind(row.get::<i64, _>("id"))
            .execute(pool)
            .await?;
        tracing::info!(key = %doc["key"], "dropped retired fields from a stored document");
    }
    Ok(())
}

/// Whether anything was removed.
fn strip(value: &mut Value) -> bool {
    match value {
        Value::Object(map) => {
            let mut hit = false;
            for key in RETIRED {
                hit |= map.remove(key).is_some();
            }
            for v in map.values_mut() {
                hit |= strip(v);
            }
            hit
        }
        Value::Array(items) => items.iter_mut().fold(false, |hit, v| hit | strip(v)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn retired_keys_go_at_every_depth() {
        let mut doc = json!({
            "key": "g", "description": "old",
            "settings": { "fields": [{ "name": "a", "ty": { "kind": "text" }, "required": true }] },
            "auths": [{ "id": "t", "kind": "query", "name": "k", "value": "settings.a" }],
            "methods": { "pay": { "enabled": true, "requests": [
                { "name": "c", "query": [], "timeout_ms": 5000,
                  "response": { "success_when": "resp.ok", "error": { "code": "resp.body.c" } } }
            ] } }
        });
        assert!(strip(&mut doc));
        assert_eq!(
            doc,
            json!({
                "key": "g",
                "settings": { "fields": [{ "name": "a" }] },
                "auths": [{ "id": "t", "kind": "query", "name": "k", "value": "settings.a" }],
                "methods": { "pay": { "requests": [
                    { "name": "c", "response": { "success_when": "resp.ok", "error": {} } }
                ] } }
            }),
            "a `query` auth kind is a value, not a key, and survives"
        );
        assert!(!strip(&mut doc));
    }
}
