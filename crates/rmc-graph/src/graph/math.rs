/// Cosine similarity between two equal-length f32 vectors.
///
/// Returns 0.0 when either vector has zero norm instead of NaN. Slices of
/// unequal length are silently truncated to the shorter length via `zip`.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0f32;
    let mut na = 0f32;
    let mut nb = 0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

#[cfg(test)]
mod tests {
    use super::cosine;

    #[test]
    fn cosine_basic_identities() {
        // identical -> 1.0
        let v = vec![1.0_f32, 2.0, 3.0];
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-6);
        // orthogonal -> 0.0
        let a = vec![1.0_f32, 0.0];
        let b = vec![0.0_f32, 1.0];
        assert!(cosine(&a, &b).abs() < 1e-6);
        // opposite -> -1.0
        let a = vec![1.0_f32, 2.0, 3.0];
        let neg: Vec<f32> = a.iter().map(|x| -x).collect();
        assert!((cosine(&a, &neg) + 1.0).abs() < 1e-6);
        // zero-norm input -> 0.0 (no NaN)
        let z = vec![0.0_f32; 3];
        assert_eq!(cosine(&z, &v), 0.0);
        assert_eq!(cosine(&v, &z), 0.0);
    }
}
