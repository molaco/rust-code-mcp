//! Stable, content-addressed identifiers for graph nodes.
//!
//! NodeIds are SHA-256 hashes derived from path-like component tuples
//! (`workspace_hash`, `kind`, `crate`, `module_path`, `item_kind`, `item_name`).
//! They are stable across rust-analyzer reloads — unlike `ModuleDefId`, which
//! is per-load — and across edits that don't rename or move the symbol.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NodeId(#[serde(with = "serde_bytes_32")] pub [u8; 32]);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BindingId(#[serde(with = "serde_bytes_32")] pub [u8; 32]);

impl BindingId {
    pub fn from_components(parts: &[&str]) -> Self {
        let mut hasher = Sha256::new();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                hasher.update(&[0u8]);
            }
            hasher.update(part.as_bytes());
        }
        let digest = hasher.finalize();
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        Self(out)
    }

    pub fn to_hex(&self) -> String {
        hex_encode(&self.0)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for BindingId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BindingId({}…)", &self.to_hex()[..12])
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct UsageId(#[serde(with = "serde_bytes_32")] pub [u8; 32]);

impl UsageId {
    pub fn from_components(parts: &[&str]) -> Self {
        let mut hasher = Sha256::new();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                hasher.update(&[0u8]);
            }
            hasher.update(part.as_bytes());
        }
        let digest = hasher.finalize();
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        Self(out)
    }

    pub fn to_hex(&self) -> String {
        hex_encode(&self.0)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for UsageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "UsageId({}…)", &self.to_hex()[..12])
    }
}

impl NodeId {
    pub fn from_components(parts: &[&str]) -> Self {
        let mut hasher = Sha256::new();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                hasher.update(&[0u8]); // unambiguous separator
            }
            hasher.update(part.as_bytes());
        }
        let digest = hasher.finalize();
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        Self(out)
    }

    pub fn to_hex(&self) -> String {
        hex_encode(&self.0)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NodeId({}…)", &self.to_hex()[..12])
    }
}

pub fn workspace_hash(workspace_root: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(workspace_root.to_string_lossy().as_bytes());
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

mod serde_bytes_32 {
    use serde::{Deserializer, Serializer, de::Error};

    pub(super) fn serialize<S: Serializer>(bytes: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        serde_bytes::serialize(&bytes[..], s)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let v: Vec<u8> = serde_bytes::deserialize(d)?;
        if v.len() != 32 {
            return Err(D::Error::custom("NodeId must be 32 bytes"));
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(&v);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_deterministic_and_distinct() {
        let a = NodeId::from_components(&["wh", "crate", "foo"]);
        let b = NodeId::from_components(&["wh", "crate", "foo"]);
        let c = NodeId::from_components(&["wh", "crate", "bar"]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn separator_prevents_collision() {
        // Without a separator, ["a", "bc"] and ["ab", "c"] would hash the same.
        let a = NodeId::from_components(&["a", "bc"]);
        let b = NodeId::from_components(&["ab", "c"]);
        assert_ne!(a, b);
    }
}
