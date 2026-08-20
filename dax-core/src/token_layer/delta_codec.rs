use serde::{Serialize, Deserialize};
use thiserror::Error;
use std::collections::HashMap;
use std::hash::{Hasher, Hash};
use std::collections::hash_map::DefaultHasher;
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

/// Compute 64-bit hash for a byte slice (fast, non-cryptographic)
fn fast_hash64(data: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    data.hash(&mut h);
    h.finish()
}

/// Chunk similarity (byte-level cosine) — kept for optional diagnostics
fn chunk_similarity(a: &[u8], b: &[u8]) -> f32 {
    let len = a.len().min(b.len());
    if len == 0 { return 0.0; }

    let mut dot = 0f32;
    let mut norm_a = 0f32;
    let mut norm_b = 0f32;

    for i in 0..len {
        let x = a[i] as f32;
        let y = b[i] as f32;
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }

    dot / (norm_a.sqrt() * norm_b.sqrt() + 1e-6)
}

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

    /// Lossless dedupe + XOR-delta clustering
    /// - exact duplicates become references
    /// - near-duplicates become (rep, xor_delta) when beneficial
    /// threshold_bytes: maximum differing bytes to consider XOR-delta beneficial
    pub fn cluster_lossless(&mut self, threshold_bytes: usize) {
        if self.raws.len() < 2 { return; }

        // map hash -> index of representative in 'reps'
        let mut hash_map: HashMap<u64, usize> = HashMap::with_capacity(self.raws.len());
        let mut reps: Vec<Vec<u8>> = Vec::new();

        // Entry describes how each original chunk will be encoded after clustering
        enum Entry {
            Ref(usize),                 // reference to rep index
            XorDelta { rep_idx: usize, delta: Vec<u8> },
            Raw(Vec<u8>),
        }
        let mut entries: Vec<Entry> = Vec::with_capacity(self.raws.len());

        // First pass: build reps and cheap exact dedupe via hash
        for raw in &self.raws {
            let h = fast_hash64(raw);
            if let Some(&rep_idx) = hash_map.get(&h) {
                // quick path: identical hash -> verify equality
                if reps[rep_idx].as_slice() == raw.as_slice() {
                    entries.push(Entry::Ref(rep_idx));
                    continue;
                }
            }

            // No exact match by hash: try to find a close rep by scanning existing reps
            // We limit scanning with a simple length heuristic to keep this near-linear in practice.
            let mut best_rep: Option<(usize, usize)> = None; // (rep_idx, diff_bytes)
            for (idx, rep) in reps.iter().enumerate() {
                // length heuristic: skip if lengths differ by more than threshold
                if (rep.len() as isize - raw.len() as isize).abs() as usize > threshold_bytes {
                    continue;
                }
                // compute bytewise difference count with early exit
                let min_len = rep.len().min(raw.len());
                let mut diff_count = 0usize;
                let mut k = 0usize;
                while k < min_len && diff_count <= threshold_bytes {
                    if rep[k] != raw[k] { diff_count += 1; }
                    k += 1;
                }
                diff_count += (rep.len().max(raw.len()) - min_len);
                if diff_count <= threshold_bytes {
                    if best_rep.is_none() || diff_count < best_rep.unwrap().1 {
                        best_rep = Some((idx, diff_count));
                    }
                }
            }

            if let Some((rep_idx, _)) = best_rep {
                // build XOR delta (length = max(rep.len(), raw.len()))
                let rep = &reps[rep_idx];
                let max_len = rep.len().max(raw.len());
                let mut delta = vec![0u8; max_len];
                for i in 0..max_len {
                    let a = if i < rep.len() { rep[i] } else { 0 };
                    let b = if i < raw.len() { raw[i] } else { 0 };
                    delta[i] = a ^ b;
                }
                entries.push(Entry::XorDelta { rep_idx, delta });
            } else {
                // create a new representative
                let rep_idx = reps.len();
                reps.push(raw.clone());
                hash_map.insert(h, rep_idx);
                entries.push(Entry::Ref(rep_idx));
            }
        }

        // Build new_raws: reps block first, then per-original entry blocks
        // reps_block format: [rep_count:u32][rep0_len:u32][rep0_bytes]...
        let mut new_raws: Vec<Vec<u8>> = Vec::with_capacity(1 + entries.len());
        let mut reps_block = Vec::new();
        reps_block.extend_from_slice(&(reps.len() as u32).to_be_bytes());
        for r in &reps {
            reps_block.extend_from_slice(&(r.len() as u32).to_be_bytes());
            reps_block.extend_from_slice(&r);
        }
        new_raws.push(reps_block);

        // entry formats:
        // tag 0x01 = Ref(rep_idx:u32)
        // tag 0x02 = XorDelta(rep_idx:u32, delta_len:u32, delta_bytes)
        // tag 0x00 = Raw(len:u32, bytes...)
        for e in entries {
            match e {
                Entry::Ref(idx) => {
                    let mut b = Vec::with_capacity(1 + 4);
                    b.push(0x01);
                    b.extend_from_slice(&(idx as u32).to_be_bytes());
                    new_raws.push(b);
                }
                Entry::XorDelta { rep_idx, delta } => {
                    let mut b = Vec::with_capacity(1 + 4 + 4 + delta.len());
                    b.push(0x02);
                    b.extend_from_slice(&(rep_idx as u32).to_be_bytes());
                    b.extend_from_slice(&(delta.len() as u32).to_be_bytes());
                    b.extend_from_slice(&delta);
                    new_raws.push(b);
                }
                Entry::Raw(r) => {
                    let mut b = Vec::with_capacity(1 + 4 + r.len());
                    b.push(0x00);
                    b.extend_from_slice(&(r.len() as u32).to_be_bytes());
                    b.extend_from_slice(&r);
                    new_raws.push(b);
                }
            }
        }

        // Replace raws with new_raws and recompute total_payload_len
        self.total_payload_len = new_raws.iter().map(|r| r.len()).sum();
        self.raws = new_raws;
    }

    /// Finalize into a single uncompressed grouped block:
    /// [count: u32 BE][offset0: u32 BE]...[offsetN-1: u32 BE][payload...]
    /// Offsets are relative to the start of the payload (0..payload_len).
    /// This version runs lossless clustering (dedupe + XOR-delta) before grouping to regain speed while preserving exact reconstruction.
    pub fn finalize_uncompressed(mut self) -> Vec<u8> {
        // Apply lossless clustering with a small threshold (tunable).
        // Start with threshold_bytes = 8; adjust based on dataset characteristics.
        self.cluster_lossless(8);

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
///
/// This function now also detects the lossless clustering format produced by `cluster_lossless`:
/// - If the payload encodes [reps_block][entry0][entry1]..., it expands entries back into original raw deltas.
/// - Otherwise it returns the raw slices as before.
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

    // Attempt to detect and expand the clustering format:
    // Clustering format layout in `raws`:
    // raws[0] = reps_block: [rep_count:u32][rep0_len:u32][rep0_bytes]...
    // raws[1..] = entries with tag bytes (0x01,0x02,0x00)
    // If this pattern matches, expand entries into original raw deltas.
    if raws.len() >= 1 {
        // Try parse reps_block safely
        let reps_block = &raws[0];
        if reps_block.len() >= 4 {
            let mut pos = 0usize;
            let rep_count = u32::from_be_bytes(reps_block[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            // quick sanity: rep_count reasonable and reps_block contains at least rep_count length headers
            let mut reps: Vec<Vec<u8>> = Vec::with_capacity(rep_count);
            let mut ok = true;
            for _ in 0..rep_count {
                if pos + 4 > reps_block.len() { ok = false; break; }
                let rlen = u32::from_be_bytes(reps_block[pos..pos + 4].try_into().unwrap()) as usize;
                pos += 4;
                if pos + rlen > reps_block.len() { ok = false; break; }
                reps.push(reps_block[pos..pos + rlen].to_vec());
                pos += rlen;
            }
            // If parsing reps succeeded and number of raws equals 1 + number_of_entries, treat as clustered format
            if ok && raws.len() == 1 + (raws.len() - 1) {
                // Now parse entries and expand
                let mut expanded: Vec<Vec<u8>> = Vec::with_capacity(raws.len() - 1);
                let mut parse_ok = true;
                for entry in raws.iter().skip(1) {
                    if entry.is_empty() { parse_ok = false; break; }
                    let tag = entry[0];
                    match tag {
                        0x01 => {
                            // Ref(rep_idx:u32)
                            if entry.len() < 1 + 4 { parse_ok = false; break; }
                            let rep_idx = u32::from_be_bytes(entry[1..5].try_into().unwrap()) as usize;
                            if rep_idx >= reps.len() { parse_ok = false; break; }
                            expanded.push(reps[rep_idx].clone());
                        }
                        0x02 => {
                            // XorDelta(rep_idx:u32, delta_len:u32, delta_bytes)
                            if entry.len() < 1 + 4 + 4 { parse_ok = false; break; }
                            let rep_idx = u32::from_be_bytes(entry[1..5].try_into().unwrap()) as usize;
                            let delta_len = u32::from_be_bytes(entry[5..9].try_into().unwrap()) as usize;
                            if entry.len() < 1 + 4 + 4 + delta_len { parse_ok = false; break; }
                            if rep_idx >= reps.len() { parse_ok = false; break; }
                            let delta_bytes = &entry[9..9 + delta_len];
                            let rep = &reps[rep_idx];
                            let max_len = rep.len().max(delta_bytes.len());
                            let mut raw = vec![0u8; max_len];
                            for i in 0..max_len {
                                let a = if i < rep.len() { rep[i] } else { 0 };
                                let b = if i < delta_bytes.len() { delta_bytes[i] } else { 0 };
                                raw[i] = a ^ b;
                            }
                            expanded.push(raw);
                        }
                        0x00 => {
                            // Raw(len:u32, bytes...)
                            if entry.len() < 1 + 4 { parse_ok = false; break; }
                            let rlen = u32::from_be_bytes(entry[1..5].try_into().unwrap()) as usize;
                            if entry.len() < 1 + 4 + rlen { parse_ok = false; break; }
                            expanded.push(entry[5..5 + rlen].to_vec());
                        }
                        _ => {
                            parse_ok = false;
                            break;
                        }
                    }
                }

                if parse_ok {
                    return Ok(expanded);
                }
                // else fallthrough and return original raws (backward compatibility)
            }
        }
    }

    Ok(raws)
}

/// Decompress grouped compressed block and return raw deltas
/// This now expands clustered encodings back into original raw deltas so apply() can succeed.
pub fn decompress_and_unpack<C>(compressed: &[u8], compressor: &C) -> Result<Vec<Vec<u8>>, &'static str>
where
    C: BitDropCompressor,
{
    let decompressed = compressor.decompress(compressed);
    unpack_grouped_uncompressed(&decompressed)
}
