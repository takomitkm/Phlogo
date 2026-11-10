//! Node service (mirrors the persistence side of `internal/node/upsert.go`).
use database::{NodeRepository, NodeRow};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct NodeService {
    repo: NodeRepository,
}

impl NodeService {
    pub fn new(repo: NodeRepository) -> Self {
        Self { repo }
    }

    pub async fn upsert_by_hash(
        &self,
        identity_hash: &str,
        fingerprint_version: i32,
        protocol: &str,
        server: &str,
        port: i32,
    ) -> anyhow::Result<(Uuid, bool)> {
        self.repo
            .upsert_by_hash(identity_hash, fingerprint_version, protocol, server, port)
            .await
    }

    pub async fn link(&self, node_id: Uuid, source_id: Uuid) -> anyhow::Result<u64> {
        self.repo.link(node_id, source_id).await
    }

    pub async fn list(&self) -> anyhow::Result<Vec<NodeRow>> {
        self.repo.list().await
    }

    pub async fn get(&self, id: Uuid) -> anyhow::Result<Option<NodeRow>> {
        self.repo.get(id).await
    }

    pub async fn delete(&self, id: Uuid) -> anyhow::Result<bool> {
        self.repo.delete(id).await
    }

    pub async fn count_node_sources(&self, id: Uuid) -> anyhow::Result<i64> {
        self.repo.count_node_sources(id).await
    }
}
