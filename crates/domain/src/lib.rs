//! Domain layer: entities + domain services.
pub mod fingerprint;
pub mod node;

pub use fingerprint::{
    compute_identity, node_auth, normalize_credential, stamp, CURRENT_FINGERPRINT_VERSION,
};
pub use node::{Credential, NormalizedNode, SourceContext, TlsSettings, Transport};
