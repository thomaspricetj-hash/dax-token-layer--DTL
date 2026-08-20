use crate::token_layer::base_tokenizer::StubTokenizer;
use crate::token_layer::delta_codec::DaxDeltaCodec;
use crate::token_layer::bitdrop_adapter::BitDropCompressor;

/// Core DAX token layer: tokenizer + delta codec + compressor
#[derive(Clone)]
pub struct DaxTokenLayer<C, D> {
    tokenizer: StubTokenizer,
    delta_codec: D,
    compressor: C,
}

impl<C, D> DaxTokenLayer<C, D>
where
    C: BitDropCompressor + Clone,
    D: DaxDeltaCodec + Clone,
{
    pub fn new(tokenizer: StubTokenizer, delta_codec: D, compressor: C) -> Self {
        Self {
            tokenizer,
            delta_codec,
            compressor,
        }
    }

    /// Normal tokenization (no delta)
    pub fn encode_only(&mut self, text: &str) -> Vec<u32> {
        self.tokenizer.encode(text)
    }

    /// Full DAX encode: tokenization + delta + compression
    pub fn encode_with_delta(&mut self, text: &str) -> (Vec<u32>, Vec<u8>) {
        let tokens = self.encode_only(text);
        let delta_bytes = self.delta_codec.diff(&tokens, &tokens);
        let compressed = self.compressor.compress(&delta_bytes);
        (tokens, compressed)
    }

    /// Reconstruct tokens from base + compressed delta
    pub fn reconstruct_tokens(
        &mut self,
        base_tokens: &[u32],
        compressed_delta: &[u8],
    ) -> Vec<u32> {
        let delta_bytes = self.compressor.decompress(compressed_delta);
        self.delta_codec
            .apply(base_tokens, &delta_bytes)
            .unwrap_or_else(|_| base_tokens.to_vec())
    }
}
