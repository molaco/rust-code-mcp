//! Stable embedding identity codec.
//!
//! This module owns the v2 identity string format only. Legacy identity
//! parsing stays in `EmbeddingBackend::from_identity` so this codec can remain
//! a small, testable data codec.

use super::backend::EmbeddingRuntime;

const PREFIX: &str = "emb";
const SCHEMA_VERSION: &str = "2";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EmbeddingIdentity {
    pub(crate) runtime: EmbeddingRuntime,
    pub(crate) model_id: String,
    pub(crate) dim: usize,
    pub(crate) max_len: usize,
    pub(crate) query: String,
}

impl EmbeddingIdentity {
    pub(crate) fn encode(&self) -> String {
        format!(
            "{PREFIX};v={SCHEMA_VERSION};rt={};model={};dim={};max={};query={}",
            encode_runtime(self.runtime),
            percent_encode(&self.model_id),
            self.dim,
            self.max_len,
            percent_encode(&self.query),
        )
    }

    pub(crate) fn decode(input: &str) -> Result<Self, String> {
        let mut fields = input.split(';');
        match fields.next() {
            Some(PREFIX) => {}
            Some(other) => {
                return Err(format!(
                    "invalid embedding identity prefix `{other}` in `{input}`"
                ));
            }
            None => {
                return Err("empty embedding identity".to_string());
            }
        }

        let mut version = None;
        let mut runtime = None;
        let mut model_id = None;
        let mut dim = None;
        let mut max_len = None;
        let mut query = None;

        for field in fields {
            let (key, value) = field.split_once('=').ok_or_else(|| {
                format!("malformed embedding identity field `{field}` in `{input}`")
            })?;
            match key {
                "v" => set_once(&mut version, value.to_string(), key, input)?,
                "rt" => set_once(&mut runtime, decode_runtime(value)?, key, input)?,
                "model" => set_once(&mut model_id, percent_decode(value)?, key, input)?,
                "dim" => set_once(&mut dim, parse_usize(value, key, input)?, key, input)?,
                "max" => set_once(&mut max_len, parse_usize(value, key, input)?, key, input)?,
                "query" => set_once(&mut query, percent_decode(value)?, key, input)?,
                _ => {}
            }
        }

        let version = required(version, "v", input)?;
        if version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported embedding identity schema version `{version}` in `{input}`"
            ));
        }

        Ok(Self {
            runtime: required(runtime, "rt", input)?,
            model_id: required(model_id, "model", input)?,
            dim: required(dim, "dim", input)?,
            max_len: required(max_len, "max", input)?,
            query: required(query, "query", input)?,
        })
    }
}

fn set_once<T>(slot: &mut Option<T>, value: T, key: &str, input: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        return Err(format!(
            "duplicate embedding identity field `{key}` in `{input}`"
        ));
    }
    Ok(())
}

fn required<T>(value: Option<T>, key: &str, input: &str) -> Result<T, String> {
    value.ok_or_else(|| format!("missing embedding identity field `{key}` in `{input}`"))
}

fn parse_usize(value: &str, key: &str, input: &str) -> Result<usize, String> {
    value.parse().map_err(|e| {
        format!("failed to parse embedding identity field `{key}` value `{value}` in `{input}`: {e}")
    })
}

fn encode_runtime(runtime: EmbeddingRuntime) -> &'static str {
    match runtime {
        EmbeddingRuntime::LocalQwen3CandleCuda => "local-qwen3-candle-cuda",
        EmbeddingRuntime::LocalFastembedOnnxCpu => "local-fastembed-onnx-cpu",
        EmbeddingRuntime::OpenRouter => "openrouter",
    }
}

fn decode_runtime(value: &str) -> Result<EmbeddingRuntime, String> {
    match value {
        "local-qwen3-candle-cuda" => Ok(EmbeddingRuntime::LocalQwen3CandleCuda),
        "local-fastembed-onnx-cpu" => Ok(EmbeddingRuntime::LocalFastembedOnnxCpu),
        "openrouter" => Ok(EmbeddingRuntime::OpenRouter),
        other => Err(format!("unknown embedding runtime `{other}`")),
    }
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if is_safe_byte(byte) {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(hex_digit(byte >> 4));
            out.push(hex_digit(byte & 0x0F));
        }
    }
    out
}

fn percent_decode(input: &str) -> Result<String, String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(format!("truncated percent escape in `{input}`"));
            }
            let high = from_hex(bytes[i + 1])
                .ok_or_else(|| format!("invalid percent escape in `{input}`"))?;
            let low = from_hex(bytes[i + 2])
                .ok_or_else(|| format!("invalid percent escape in `{input}`"))?;
            out.push((high << 4) | low);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }

    String::from_utf8(out).map_err(|e| format!("invalid UTF-8 in percent escape: {e}"))
}

fn is_safe_byte(byte: u8) -> bool {
    matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-')
}

fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        10..=15 => char::from(b'A' + (nibble - 10)),
        _ => unreachable!("nibble is masked to 4 bits"),
    }
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_with_reserved_strings() -> EmbeddingIdentity {
        EmbeddingIdentity {
            runtime: EmbeddingRuntime::OpenRouter,
            model_id: "provider/model:with=reserved; chars\nnext".to_string(),
            dim: 1024,
            max_len: 4096,
            query: "input=query;document=doc\nwith spaces".to_string(),
        }
    }

    #[test]
    fn round_trips_reserved_string_fields() {
        let identity = identity_with_reserved_strings();
        let encoded = identity.encode();
        let decoded = EmbeddingIdentity::decode(&encoded).unwrap();

        assert_eq!(decoded, identity);
    }

    #[test]
    fn encoded_identity_uses_filesystem_safe_ascii() {
        let encoded = identity_with_reserved_strings().encode();

        assert!(encoded.bytes().all(|byte| {
            matches!(
                byte,
                b'A'..=b'Z'
                    | b'a'..=b'z'
                    | b'0'..=b'9'
                    | b'.'
                    | b'_'
                    | b'-'
                    | b'%'
                    | b';'
                    | b'='
            )
        }));
    }

    #[test]
    fn decode_is_order_independent() {
        let input = "emb;query=q;max=512;model=openrouter%2Fmodel;rt=openrouter;dim=384;v=2";
        let decoded = EmbeddingIdentity::decode(input).unwrap();

        assert_eq!(decoded.runtime, EmbeddingRuntime::OpenRouter);
        assert_eq!(decoded.model_id, "openrouter/model");
        assert_eq!(decoded.dim, 384);
        assert_eq!(decoded.max_len, 512);
        assert_eq!(decoded.query, "q");
    }

    #[test]
    fn decode_rejects_malformed_input() {
        assert!(EmbeddingIdentity::decode("").is_err());
        assert!(EmbeddingIdentity::decode("bad;v=2").is_err());
        assert!(EmbeddingIdentity::decode("emb;v=2;rt=openrouter").is_err());
        assert!(EmbeddingIdentity::decode(
            "emb;v=2;rt=openrouter;model=a;dim=1;max=2;query=%ZZ"
        )
        .is_err());
        assert!(EmbeddingIdentity::decode(
            "emb;v=2;rt=openrouter;model=a;dim=1;max=2;query=a;query=b"
        )
        .is_err());
    }

    #[test]
    fn decode_rejects_unknown_schema_versions() {
        assert!(EmbeddingIdentity::decode(
            "emb;v=3;rt=openrouter;model=a;dim=1;max=2;query=q"
        )
        .is_err());
    }
}
