use std::sync::Arc;

pub(in crate::embeddings) fn arc(value: &str) -> Arc<str> {
    Arc::<str>::from(value)
}
