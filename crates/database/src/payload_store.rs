//! Encrypted node payload storage (M1).
//!
//! Only sealed bytes are persisted; decryption belongs to the caller
//! (`payload_crypto::global().open(...)` with the node id as AAD).
use payload_crypto::Sealed;
use sqlx::PgConnection;
use uuid::Uuid;

/// A sealed payload as read back from `node_payloads`.
#[derive(Debug, Clone)]
pub struct StoredPayload {
    pub key_version: i32,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

/// Insert or replace the sealed payload of one node.
pub async fn upsert_node_payload(
    conn: &mut PgConnection,
    node_id: Uuid,
    sealed: &Sealed,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO node_payloads (node_id,key_version,nonce,ciphertext,updated_at) \
         VALUES ($1,$2,$3,$4,now()) \
         ON CONFLICT (node_id) DO UPDATE SET \
           key_version = EXCLUDED.key_version, \
           nonce = EXCLUDED.nonce, \
           ciphertext = EXCLUDED.ciphertext, \
           updated_at = now()",
    )
    .bind(node_id)
    .bind(sealed.key_version)
    .bind(sealed.nonce.to_vec())
    .bind(sealed.ciphertext.clone())
    .execute(conn)
    .await?;
    Ok(())
}

/// Load one node's sealed payload (no decryption here).
pub async fn load_node_payload(
    conn: &mut PgConnection,
    node_id: Uuid,
) -> sqlx::Result<Option<StoredPayload>> {
    let row: Option<(i32, Vec<u8>, Vec<u8>)> =
        sqlx::query_as("SELECT key_version,nonce,ciphertext FROM node_payloads WHERE node_id=$1")
            .bind(node_id)
            .fetch_optional(conn)
            .await?;
    Ok(row.map(|(key_version, nonce, ciphertext)| StoredPayload {
        key_version,
        nonce,
        ciphertext,
    }))
}
