//! Domain layer: entities + domain services.
pub mod node;

pub use node::{Credential, NormalizedNode, SourceContext, TlsSettings, Transport};
