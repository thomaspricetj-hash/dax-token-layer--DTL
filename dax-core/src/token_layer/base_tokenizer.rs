use serde::{Serialize, Deserialize};

/// A simple deterministic tokenizer for benchmarking.
/// Splits on whitespace and assigns stable token IDs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StubTokenizer;

impl StubTokenizer {
    pub fn encode(&self, text: &str) -> Vec<u32> {
        // Very simple tokenizer:
        // - split on whitespace
        // - hash each token into a u32
        // - stable across runs
        text.split_whitespace()
            .map(|tok| stable_hash(tok))
            .collect()
    }
}

/// Stable 32‑bit hash for tokens.
/// This ensures delta codecs behave predictably.
fn stable_hash(s: &str) -> u32 {
    // FNV‑1a 32‑bit (fast, stable, reversible enough for testing)
    const FNV_OFFSET: u32 = 0x811C9DC5;
    const FNV_PRIME: u32 = 0x01000193;

    let mut hash = FNV_OFFSET;
    for b in s.as_bytes() {
        hash ^= *b as u32;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}
