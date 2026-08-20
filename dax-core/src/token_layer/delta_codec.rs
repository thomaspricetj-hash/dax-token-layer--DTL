use serde::{Serialize, Deserialize};
use thiserror::Error;
use crate::token_layer::bitdrop_adapter::BitDropCompressor;

#[derive(Debug, Error)]
pub enum DeltaError {
    #[error("delta decode failed")]
    DecodeFailed,
}

/// Core trait for all delta codecs
pub trait DaxDeltaCodec {
    fn diff(&self, base: &[u32], current: &[u32]) -> Vec<u8>;
    fn apply(&self, base: &[u32], delta: &[u8]) -> Result<Vec<u32>, DeltaError>;
}

//
// SIMPLE DAX DELTA
//
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SimpleDaxDeltaCodec;

impl DaxDeltaCodec for SimpleDaxDeltaCodec {
    fn diff(&self, _base: &[u32], current: &[u32]) -> Vec<u8> {
        let len = current.len() as u32;
        let mut buf = Vec::with_capacity(4 + len as usize * 4);
        buf.extend_from_slice(&len.to_le_bytes());
        for &t in current {
            buf.extend_from_slice(&t.to_le_bytes());
        }
        buf
    }

    fn apply(&self, _base: &[u32], delta: &[u8]) -> Result<Vec<u32>, DeltaError> {
        if delta.len() < 4 { return Err(DeltaError::DecodeFailed); }
        let len = u32::from_le_bytes(delta[0..4].try_into().unwrap()) as usize;
        if delta.len() < 4 + len * 4 { return Err(DeltaError::DecodeFailed); }
        let mut out = Vec::with_capacity(len);
        let mut offset = 4;
        for _ in 0..len {
            let v = u32::from_le_bytes(delta[offset..offset + 4].try_into().unwrap());
            out.push(v);
            offset += 4;
        }
        Ok(out)
    }
}

//
// DAX MASTER DELTA (XOR)
//
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DaxMasterDeltaCodec;

impl DaxDeltaCodec for DaxMasterDeltaCodec {
    fn diff(&self, base: &[u32], current: &[u32]) -> Vec<u8> {
        let len = current.len() as u32;
        let mut buf = Vec::with_capacity(4 + current.len() * 4);
        buf.extend_from_slice(&len.to_le_bytes());
        for i in 0..current.len() {
            let b = base.get(i).copied().unwrap_or(0);
            let c = current[i];
            let d = b ^ c;
            buf.extend_from_slice(&d.to_le_bytes());
        }
        buf
    }

    fn apply(&self, base: &[u32], delta: &[u8]) -> Result<Vec<u32>, DeltaError> {
        if delta.len() < 4 { return Err(DeltaError::DecodeFailed); }
        let len = u32::from_le_bytes(delta[0..4].try_into().unwrap()) as usize;
        if delta.len() < 4 + len * 4 { return Err(DeltaError::DecodeFailed); }
        let mut out = Vec::with_capacity(len);
        let mut offset = 4;
        for i in 0..len {
            let d = u32::from_le_bytes(delta[offset..offset + 4].try_into().unwrap());
            let b = base.get(i).copied().unwrap_or(0);
            let c = b ^ d;
            out.push(c);
            offset += 4;
        }
        Ok(out)
    }
}

//
// GROUPED DELTA CODEC
//
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupedDaxDeltaCodec<C> {
    pub compressor: C,
    pub group_size: usize,
}

impl<C> GroupedDaxDeltaCodec<C>
where
    C: Clone + Send + Sync + 'static,
{
    pub fn new(compressor: C, group_size: usize) -> Self {
        Self { compressor, group_size }
    }
}

impl<C> DaxDeltaCodec for GroupedDaxDeltaCodec<C>
where
    C: BitDropCompressor + Clone + Send + Sync + 'static,
{
    fn diff(&self, base: &[u32], current: &[u32]) -> Vec<u8> {
        let n = base.len().min(current.len());
        let mut pairs = Vec::new();
        for i in 0..n {
            if base[i] != current[i] {
                pairs.push((i as u32, current[i]));
            }
        }
        let mut raw = Vec::with_capacity(pairs.len() * 8 + 4);
        raw.extend_from_slice(&(pairs.len() as u32).to_be_bytes());
        for (idx, val) in pairs {
            raw.extend_from_slice(&idx.to_be_bytes());
            raw.extend_from_slice(&val.to_be_bytes());
        }
        self.compressor.compress(&raw)
    }

    fn apply(&self, base: &[u32], delta: &[u8]) -> Result<Vec<u32>, DeltaError> {
        let decompressed = self.compressor.decompress(delta);
        if decompressed.len() < 4 { return Err(DeltaError::DecodeFailed); }
        let mut offset = 0usize;
        let count = u32::from_be_bytes(decompressed[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        let mut out = base.to_vec();
        for _ in 0..count {
            if offset + 8 > decompressed.len() { return Err(DeltaError::DecodeFailed); }
            let idx = u32::from_be_bytes(decompressed[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            let val = u32::from_be_bytes(decompressed[offset..offset + 4].try_into().unwrap());
            offset += 4;
            if idx < out.len() { out[idx] = val; }
        }
        Ok(out)
    }
}

//
// INDEXED + GROUPED DELTA CODEC
//
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexedGroupedDaxDeltaCodec<C> {
    pub compressor: C,
    pub group_size: usize,
}

impl<C> IndexedGroupedDaxDeltaCodec<C>
where
    C: Clone + Send + Sync + 'static,
{
    pub fn new(compressor: C, group_size: usize) -> Self {
        Self { compressor, group_size }
    }
}

impl<C> DaxDeltaCodec for IndexedGroupedDaxDeltaCodec<C>
where
    C: BitDropCompressor + Clone + Send + Sync + 'static,
{
    fn diff(&self, base: &[u32], current: &[u32]) -> Vec<u8> {
        let n = base.len().min(current.len());
        let mut pairs = Vec::new();
        for i in 0..n {
            if base[i] != current[i] {
                pairs.push((i as u32, current[i]));
            }
        }
        let mut raw = Vec::with_capacity(pairs.len() * 8 + 4);
        raw.extend_from_slice(&(pairs.len() as u32).to_be_bytes());
        for (idx, val) in pairs {
            raw.extend_from_slice(&idx.to_be_bytes());
            raw.extend_from_slice(&val.to_be_bytes());
        }
        raw
    }

    fn apply(&self, base: &[u32], delta: &[u8]) -> Result<Vec<u32>, DeltaError> {
        let decompressed = self.compressor.decompress(delta);
        if decompressed.len() < 8 { return Err(DeltaError::DecodeFailed); }
        let mut offset = 0usize;
        let count = u32::from_be_bytes(decompressed[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        let mut offsets = Vec::with_capacity(count);
        for _ in 0..count {
            let off = u32::from_be_bytes(decompressed[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            offsets.push(off);
        }
        let payload = &decompressed[offset..];
        let my_offset = offsets[0];
        let next_offset = offsets.get(1).copied().unwrap_or(payload.len());
        if next_offset > payload.len() || my_offset > next_offset { return Err(DeltaError::DecodeFailed); }
        let slice = &payload[my_offset..next_offset];
        if slice.len() < 4 { return Err(DeltaError::DecodeFailed); }
        let mut pos = 0;
        let pair_count = u32::from_be_bytes(slice[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        let mut out = base.to_vec();
        for _ in 0..pair_count {
            if pos + 8 > slice.len() { return Err(DeltaError::DecodeFailed); }
            let idx = u32::from_be_bytes(slice[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            let val = u32::from_be_bytes(slice[pos..pos + 4].try_into().unwrap());
            pos += 4;
            if idx < out.len() { out[idx] = val; }
        }
        Ok(out)
    }
}

//
// Group indexer utilities (build indexed grouped block and unpack)
//

/// Build a grouped, indexed block from many raw deltas.
/// Each `raw` is the per-delta bytes produced by your DaxDeltaCodec::diff.
/// Layout:
/// [count: u32 BE][offset0: u32 BE]...[offsetN-1: u32 BE][payload...]
pub struct GroupIndexer {
    raws: Vec<Vec<u8>>,
    total_payload_len: usize,
}

impl GroupIndexer {
    pub fn new() -> Self {
        Self { raws: Vec::new(), total_payload_len: 0 }
    }

    pub fn add(&mut self, raw: Vec<u8>) {
        self.total_payload_len += raw.len();
        self.raws.push(raw);
    }

    /// Finalize into a single uncompressed grouped block:
    /// [count: u32 BE][offset0: u32 BE]...[offsetN-1: u32 BE][payload...]
    /// Offsets are relative to the start of the payload (0..payload_len).
    pub fn finalize_uncompressed(self) -> Vec<u8> {
        let count = self.raws.len() as u32;
        let mut out = Vec::with_capacity(4 + 4 * self.raws.len() + self.total_payload_len);

        // count
        out.extend_from_slice(&count.to_be_bytes());

        // offsets table
        let mut offset = 0usize;
        for raw in &self.raws {
            out.extend_from_slice(&(offset as u32).to_be_bytes());
            offset += raw.len();
        }

        // payload
        for raw in self.raws {
            out.extend_from_slice(&raw);
        }

        out
    }

    /// Finalize and compress with provided compressor
    pub fn finalize_and_compress<C>(self, compressor: &C) -> Vec<u8>
    where
        C: BitDropCompressor,
    {
        let block = self.finalize_uncompressed();
        compressor.compress(&block)
    }
}

/// Unpack a grouped block (already decompressed) into the vector of raw deltas.
/// Returns Err if the block is malformed.
pub fn unpack_grouped_uncompressed(block: &[u8]) -> Result<Vec<Vec<u8>>, &'static str> {
    if block.len() < 4 {
        return Err("group block too small");
    }

    let mut offset = 0usize;
    let count = u32::from_be_bytes(block[offset..offset + 4].try_into().unwrap()) as usize;
    offset += 4;

    // need count * 4 bytes for offsets
    if block.len() < offset + count * 4 {
        return Err("group block missing offsets");
    }

    let mut offsets = Vec::with_capacity(count);
    for _ in 0..count {
        let off = u32::from_be_bytes(block[offset..offset + 4].try_into().unwrap()) as usize;
        offsets.push(off);
        offset += 4;
    }

    let payload = &block[offset..];
    let mut raws = Vec::with_capacity(count);

    for i in 0..count {
        let start = offsets[i];
        let end = if i + 1 < count { offsets[i + 1] } else { payload.len() };
        if start > end || end > payload.len() {
            return Err("invalid offsets");
        }
        raws.push(payload[start..end].to_vec());
    }

    Ok(raws)
}

/// Decompress grouped compressed block and return raw deltas
pub fn decompress_and_unpack<C>(compressed: &[u8], compressor: &C) -> Result<Vec<Vec<u8>>, &'static str>
where
    C: BitDropCompressor,
{
    let decompressed = compressor.decompress(compressed);
    unpack_grouped_uncompressed(&decompressed)
}

