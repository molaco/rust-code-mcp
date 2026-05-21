//! Response DTOs and decoding helpers for the OpenRouter embeddings endpoint.

use crate::embeddings::Embedding;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct EmbeddingResponse {
    pub(super) data: Vec<EmbeddingResponseItem>,
}

#[derive(Debug, Deserialize)]
pub(super) struct EmbeddingResponseItem {
    pub(super) embedding: EmbeddingResponseEmbedding,
    pub(super) index: usize,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum EmbeddingResponseEmbedding {
    Float(Vec<f32>),
    Base64(String),
}

impl EmbeddingResponseEmbedding {
    pub(super) fn into_embedding(self) -> Result<Embedding, String> {
        match self {
            Self::Float(embedding) => Ok(embedding),
            Self::Base64(encoded) => decode_base64_f32_embedding(&encoded),
        }
    }
}

pub(super) fn parse_embeddings_response(
    body: &str,
    expected_dim: usize,
    expected_count: usize,
) -> Result<Vec<Embedding>, String> {
    let response: EmbeddingResponse = serde_json::from_str(body)
        .map_err(|e| format!("OpenRouter embeddings response was not valid JSON: {e}"))?;
    if response.data.len() != expected_count {
        return Err(format!(
            "OpenRouter returned {} embeddings for {} inputs",
            response.data.len(),
            expected_count
        ));
    }

    let mut output: Vec<Option<Embedding>> = vec![None; expected_count];
    for item in response.data {
        if item.index >= expected_count {
            return Err(format!(
                "OpenRouter returned out-of-range embedding index {} for {} inputs",
                item.index, expected_count
            ));
        }
        let embedding = item.embedding.into_embedding().map_err(|err| {
            format!(
                "OpenRouter returned invalid base64 embedding at index {}: {err}",
                item.index
            )
        })?;
        if embedding.len() != expected_dim {
            return Err(format!(
                "OpenRouter returned embedding dimension {} at index {}, expected {}",
                embedding.len(),
                item.index,
                expected_dim
            ));
        }
        output[item.index] = Some(embedding);
    }

    output
        .into_iter()
        .enumerate()
        .map(|(idx, maybe)| {
            maybe.ok_or_else(|| {
                format!("OpenRouter response omitted embedding for input index {idx}")
            })
        })
        .collect()
}

fn decode_base64_f32_embedding(encoded: &str) -> Result<Vec<f32>, String> {
    let bytes = decode_base64_standard(encoded)?;
    if bytes.len() % 4 != 0 {
        return Err(format!(
            "decoded byte length {} is not divisible by 4",
            bytes.len()
        ));
    }

    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

fn decode_base64_standard(encoded: &str) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut quartet = [0u8; 4];
    let mut quartet_len = 0usize;
    let mut saw_padding = false;

    for byte in encoded.bytes() {
        if byte.is_ascii_whitespace() {
            continue;
        }
        if saw_padding && byte != b'=' {
            return Err("non-padding character after base64 padding".to_string());
        }

        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => {
                saw_padding = true;
                64
            }
            _ => {
                return Err(format!("invalid base64 byte 0x{byte:02x}"));
            }
        };

        quartet[quartet_len] = value;
        quartet_len += 1;

        if quartet_len == 4 {
            let padding = quartet.iter().filter(|value| **value == 64).count();
            if padding > 2 {
                return Err("invalid base64 padding length".to_string());
            }
            if quartet[0] == 64 || quartet[1] == 64 {
                return Err("invalid base64 padding position".to_string());
            }
            if padding == 1 && quartet[3] != 64 {
                return Err("invalid base64 padding position".to_string());
            }
            if padding == 2 && (quartet[2] != 64 || quartet[3] != 64) {
                return Err("invalid base64 padding position".to_string());
            }

            let b0 = quartet[0] as u32;
            let b1 = quartet[1] as u32;
            let b2 = if quartet[2] == 64 { 0 } else { quartet[2] as u32 };
            let b3 = if quartet[3] == 64 { 0 } else { quartet[3] as u32 };
            let triple = (b0 << 18) | (b1 << 12) | (b2 << 6) | b3;

            output.push(((triple >> 16) & 0xff) as u8);
            if padding < 2 {
                output.push(((triple >> 8) & 0xff) as u8);
            }
            if padding == 0 {
                output.push((triple & 0xff) as u8);
            }

            quartet_len = 0;
            quartet = [0; 4];
        }
    }

    if quartet_len != 0 {
        return Err("incomplete base64 quartet".to_string());
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_embeddings_response_in_index_order() {
        let body = r#"{
            "data": [
                {"embedding": [3.0, 4.0], "index": 1},
                {"embedding": [1.0, 2.0], "index": 0}
            ],
            "model": "qwen/qwen3-embedding-8b",
            "object": "list"
        }"#;

        let embeddings = parse_embeddings_response(body, 2, 2).unwrap();

        assert_eq!(embeddings, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
    }

    #[test]
    fn rejects_dimension_mismatch() {
        let body = r#"{
            "data": [
                {"embedding": [1.0], "index": 0}
            ]
        }"#;

        let err = parse_embeddings_response(body, 2, 1).unwrap_err();

        assert!(err.contains("embedding dimension 1"));
    }

    #[test]
    fn parses_base64_embeddings_response() {
        let body = r#"{
            "data": [
                {"embedding": "AACAPwAAAEA=", "index": 0}
            ]
        }"#;

        let embeddings = parse_embeddings_response(body, 2, 1).unwrap();

        assert_eq!(embeddings, vec![vec![1.0, 2.0]]);
    }

    #[test]
    fn rejects_invalid_base64_embeddings_response() {
        let body = r#"{
            "data": [
                {"embedding": "???", "index": 0}
            ]
        }"#;

        let err = parse_embeddings_response(body, 2, 1).unwrap_err();

        assert!(err.contains("invalid base64"));
    }
}
