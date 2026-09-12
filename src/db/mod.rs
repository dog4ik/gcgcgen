//! SQLite storage for integration documents.

pub mod integrations;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;

pub use integrations::{Repo, RepoError};

/// Opens the pool and applies migrations, creating the file if needed.
pub async fn connect(url: &str) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::from_str(url)?
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

/// An in-memory database with migrations applied, for tests.
#[cfg(test)]
pub async fn test_pool() -> SqlitePool {
    connect("sqlite::memory:")
        .await
        .expect("in-memory database")
}
