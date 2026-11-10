//! Source service (mirrors `internal/source/service.go`).
use database::{NodeRow, SourceRepository, SourceRow};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SourceService {
    repo: SourceRepository,
}

#[derive(Debug, Clone)]
pub struct CreatedSource {
    pub id: Uuid,
    pub already_existed: bool,
}

impl SourceService {
    pub fn new(repo: SourceRepository) -> Self {
        Self { repo }
    }

    pub async fn create(
        &self,
        typ: &str,
        identifier: &str,
        info: &str,
        content: &str,
    ) -> anyhow::Result<CreatedSource> {
        let (id, already_existed) = self
            .repo
            .create_dedup(typ, identifier, info, content)
            .await?;
        Ok(CreatedSource {
            id,
            already_existed,
        })
    }

    pub async fn list(&self) -> anyhow::Result<Vec<SourceRow>> {
        self.repo.list().await
    }

    pub async fn get(&self, id: Uuid) -> anyhow::Result<Option<SourceRow>> {
        self.repo.get(id).await
    }

    pub async fn delete(&self, id: Uuid) -> anyhow::Result<bool> {
        self.repo.delete(id).await
    }

    pub async fn set_ignore_dead(&self, id: Uuid, ignore: bool) -> anyhow::Result<bool> {
        self.repo.set_ignore_dead(id, ignore).await
    }

    pub async fn update(
        &self,
        id: Uuid,
        status: Option<&str>,
        identifier: Option<&str>,
        info: Option<&str>,
        content: Option<&str>,
    ) -> anyhow::Result<Option<SourceRow>> {
        self.repo
            .update(id, status, identifier, info, content)
            .await
    }

    pub async fn nodes_of_source(&self, id: Uuid) -> anyhow::Result<Vec<NodeRow>> {
        self.repo.nodes_of_source(id).await
    }
}
