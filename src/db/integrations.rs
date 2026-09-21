use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;

use crate::model::{IntegrationSummary, VersionInfo};
use crate::spec::{validate, Integration};

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("no integration with key `{0}`")]
    NotFound(String),
    #[error("version {version} of `{key}` does not exist")]
    NoSuchVersion { key: String, version: i64 },
    #[error("integration is invalid:\n{0}")]
    Invalid(#[from] validate::Report),
    #[error("stored integration `{key}` could not be parsed: {source}")]
    Corrupt {
        key: String,
        source: serde_json::Error,
    },
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Serialize(#[from] serde_json::Error),
}

#[derive(Clone)]
pub struct Repo {
    pool: SqlitePool,
}

impl Repo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn list(&self) -> Result<Vec<IntegrationSummary>, RepoError> {
        let rows = sqlx::query(
            "select key, name, version, updated_at, spec from integrations order by key",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|r| {
                let key: String = r.get("key");
                let spec: String = r.get("spec");
                let doc: Integration =
                    serde_json::from_str(&spec).map_err(|source| RepoError::Corrupt {
                        key: key.clone(),
                        source,
                    })?;
                Ok(IntegrationSummary {
                    key,
                    name: r.get("name"),
                    version: r.get("version"),
                    updated_at: r.get("updated_at"),
                    methods: doc
                        .methods
                        .iter()
                        .filter(|(_, m)| m.enabled && !m.requests.is_empty())
                        .map(|(k, _)| *k)
                        .collect(),
                })
            })
            .collect()
    }

    pub async fn load(&self, key: &str) -> Result<Integration, RepoError> {
        let row = sqlx::query("select spec from integrations where key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| RepoError::NotFound(key.to_string()))?;
        parse(key, row.get("spec"))
    }

    pub async fn try_load(&self, key: &str) -> Result<Option<Integration>, RepoError> {
        match self.load(key).await {
            Ok(doc) => Ok(Some(doc)),
            Err(RepoError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Validates, then inserts or updates, appending a version row. Returns
    /// the new version number.
    pub async fn save(&self, doc: &Integration, note: Option<&str>) -> Result<i64, RepoError> {
        validate::validate(doc)?;
        let spec = serde_json::to_string_pretty(doc)?;
        let now = now();

        let mut tx = self.pool.begin().await?;

        let existing: Option<(i64, i64)> =
            sqlx::query("select id, version from integrations where key = ?")
                .bind(&doc.key)
                .fetch_optional(&mut *tx)
                .await?
                .map(|r| (r.get("id"), r.get("version")));

        let (id, version) = match existing {
            Some((id, version)) => {
                let version = version + 1;
                sqlx::query(
                    "update integrations set name = ?, spec = ?, version = ?, updated_at = ? \
                     where id = ?",
                )
                .bind(&doc.name)
                .bind(&spec)
                .bind(version)
                .bind(&now)
                .bind(id)
                .execute(&mut *tx)
                .await?;
                (id, version)
            }
            None => {
                let id = sqlx::query(
                    "insert into integrations (key, name, spec, version, created_at, updated_at) \
                     values (?, ?, ?, 1, ?, ?) returning id",
                )
                .bind(&doc.key)
                .bind(&doc.name)
                .bind(&spec)
                .bind(&now)
                .bind(&now)
                .fetch_one(&mut *tx)
                .await?
                .get("id");
                (id, 1)
            }
        };

        sqlx::query(
            "insert into integration_versions (integration_id, version, spec, created_at, note) \
             values (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(version)
        .bind(&spec)
        .bind(&now)
        .bind(note)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(version)
    }

    pub async fn delete(&self, key: &str) -> Result<(), RepoError> {
        let done = sqlx::query("delete from integrations where key = ?")
            .bind(key)
            .execute(&self.pool)
            .await?;
        if done.rows_affected() == 0 {
            return Err(RepoError::NotFound(key.to_string()));
        }
        Ok(())
    }

    pub async fn versions(&self, key: &str) -> Result<Vec<VersionInfo>, RepoError> {
        let rows = sqlx::query(
            "select v.version, v.created_at, v.note from integration_versions v \
             join integrations i on i.id = v.integration_id \
             where i.key = ? order by v.version desc",
        )
        .bind(key)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| VersionInfo {
                version: r.get("version"),
                created_at: r.get("created_at"),
                note: r.get("note"),
            })
            .collect())
    }

    pub async fn load_version(&self, key: &str, version: i64) -> Result<Integration, RepoError> {
        let row = sqlx::query(
            "select v.spec from integration_versions v \
             join integrations i on i.id = v.integration_id \
             where i.key = ? and v.version = ?",
        )
        .bind(key)
        .bind(version)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| RepoError::NoSuchVersion {
            key: key.to_string(),
            version,
        })?;
        parse(key, row.get("spec"))
    }

    /// Republishes an old version as a new one, so history stays append-only.
    pub async fn rollback(&self, key: &str, version: i64) -> Result<i64, RepoError> {
        let doc = self.load_version(key, version).await?;
        self.save(&doc, Some(&format!("rollback to v{version}")))
            .await
    }
}

fn parse(key: &str, spec: String) -> Result<Integration, RepoError> {
    serde_json::from_str(&spec).map_err(|source| RepoError::Corrupt {
        key: key.to_string(),
        source,
    })
}

fn now() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use connect::MethodKind;

    fn doc(key: &str) -> Integration {
        serde_json::from_str(&format!(
            r#"{{
                "key": "{key}", "name": "Test", "base_url": "'https://api.example.com'",
                "settings": {{ "fields": [{{ "name": "client_id" }}] }},
                "methods": {{ "pay": {{
                    "requests": [{{ "name": "c", "path": "'/c'" }}],
                    "result": {{ "status": "\"pending\"" }} }} }}
            }}"#
        ))
        .unwrap()
    }

    #[sqlx::test]
    async fn saves_loads_and_lists(pool: SqlitePool) {
        let repo = Repo::new(pool);
        assert_eq!(repo.save(&doc("a"), None).await.unwrap(), 1);
        assert_eq!(repo.save(&doc("b"), None).await.unwrap(), 1);

        assert_eq!(repo.load("a").await.unwrap().key, "a");
        let list = repo.list().await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].key, "a");
        assert_eq!(list[0].methods, vec![MethodKind::Pay]);
    }

    #[sqlx::test]
    async fn saving_again_bumps_the_version_and_keeps_history(pool: SqlitePool) {
        let repo = Repo::new(pool);
        repo.save(&doc("a"), Some("first")).await.unwrap();

        let mut v2 = doc("a");
        v2.name = "Renamed".into();
        assert_eq!(repo.save(&v2, Some("rename")).await.unwrap(), 2);

        assert_eq!(repo.load("a").await.unwrap().name, "Renamed");
        let versions = repo.versions("a").await.unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].version, 2, "newest first");
        assert_eq!(versions[0].note.as_deref(), Some("rename"));
        assert_eq!(repo.load_version("a", 1).await.unwrap().name, "Test");
    }

    #[sqlx::test]
    async fn rollback_appends_rather_than_rewrites(pool: SqlitePool) {
        let repo = Repo::new(pool);
        repo.save(&doc("a"), None).await.unwrap();
        let mut v2 = doc("a");
        v2.name = "Broken".into();
        repo.save(&v2, None).await.unwrap();

        assert_eq!(repo.rollback("a", 1).await.unwrap(), 3);
        assert_eq!(repo.load("a").await.unwrap().name, "Test");
        assert_eq!(
            repo.versions("a").await.unwrap().len(),
            3,
            "history is append-only"
        );
    }

    #[sqlx::test]
    async fn an_invalid_document_is_never_stored(pool: SqlitePool) {
        let repo = Repo::new(pool);
        let mut bad = doc("a");
        bad.methods.get_mut(&MethodKind::Pay).unwrap().requests[0].path =
            crate::spec::Expr::literal("https://elsewhere.example/x");

        let err = repo.save(&bad, None).await.unwrap_err();
        assert!(matches!(err, RepoError::Invalid(_)), "{err}");
        assert!(repo.try_load("a").await.unwrap().is_none());
    }

    #[sqlx::test]
    async fn missing_things_report_which_thing(pool: SqlitePool) {
        let repo = Repo::new(pool);
        assert!(matches!(repo.load("nope").await, Err(RepoError::NotFound(k)) if k == "nope"));
        assert!(repo.try_load("nope").await.unwrap().is_none());
        assert!(matches!(
            repo.delete("nope").await,
            Err(RepoError::NotFound(_))
        ));

        repo.save(&doc("a"), None).await.unwrap();
        assert!(matches!(
            repo.load_version("a", 9).await,
            Err(RepoError::NoSuchVersion { version: 9, .. })
        ));
    }

    #[sqlx::test]
    async fn deleting_takes_the_history_with_it(pool: SqlitePool) {
        let repo = Repo::new(pool);
        repo.save(&doc("a"), None).await.unwrap();
        repo.save(&doc("a"), None).await.unwrap();
        repo.delete("a").await.unwrap();
        assert!(repo.versions("a").await.unwrap().is_empty());
        assert!(repo.list().await.unwrap().is_empty());
    }
}
