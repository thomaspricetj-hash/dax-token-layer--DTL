use serde::{Serialize, Deserialize};
use flate2::{Compression, write::ZlibEncoder, read::ZlibDecoder};
use std::io::{Read, Write};
use byteorder::{WriteBytesExt, ReadBytesExt, LittleEndian};
use std::cell::RefCell;
use std::convert::TryInto;
use std::time::Instant;

// Your engine crate
use bitdrop_v2::BitDrop3DEngine;

/// Trait for all BitDrop compressors
pub trait BitDropCompressor {
    fn compress(&self, input: &[u8]) -> Vec<u8>;
    fn decompress(&self, input: &[u8]) -> Vec<u8>;

    fn compress_into(&self, input: &[u8], out: &mut Vec<u8>) {
        out.clear();
        out.extend_from_slice(&self.compress(input));
    }

    fn decompress_into(&self, input: &[u8], out: &mut Vec<u8>) {
        out.clear();
        out.extend_from_slice(&self.decompress(input));
    }
}

//
// ──────────────────────────────────────────────────────────────
//   BITDROP V3 — REAL DEFLATE‑STYLE COMPRESSION (ZLIB)
// ──────────────────────────────────────────────────────────────
//

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BitDropV3;

impl BitDropCompressor for BitDropV3 {
    fn compress(&self, input: &[u8]) -> Vec<u8> {
        if input.is_empty() { return Vec::new(); }
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(input).ok();
        encoder.finish().unwrap_or_else(|_| input.to_vec())
    }

    fn decompress(&self, input: &[u8]) -> Vec<u8> {
        if input.is_empty() { return Vec::new(); }
        let mut decoder = ZlibDecoder::new(input);
        let mut out = Vec::new();
        decoder.read_to_end(&mut out).unwrap_or(0);
        out
    }
}

//
// ──────────────────────────────────────────────────────────────
//   BITDROP V5 — VARINT‑STYLE BIT‑PACKING FOR U32 STREAMS
// ──────────────────────────────────────────────────────────────
//

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BitDropV5;

impl BitDropCompressor for BitDropV5 {
    fn compress(&self, input: &[u8]) -> Vec<u8> {
        if input.len() < 4 { return input.to_vec(); }
        let mut out = Vec::new();
        let mut cursor = std::io::Cursor::new(input);
        while let Ok(v) = cursor.read_u32::<LittleEndian>() {
            write_varint_u32(&mut out, v);
        }
        out
    }

    fn decompress(&self, input: &[u8]) -> Vec<u8> {
        if input.is_empty() { return Vec::new(); }
        let mut out = Vec::new();
        let mut cursor = std::io::Cursor::new(input);
        while let Ok(v) = read_varint_u32(&mut cursor) {
            out.write_u32::<LittleEndian>(v).ok();
        }
        out
    }
}

fn write_varint_u32(buf: &mut Vec<u8>, mut v: u32) {
    while v >= 0x80 {
        buf.push(((v as u8) & 0x7F) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

fn read_varint_u32<R: Read>(r: &mut R) -> std::io::Result<u32> {
    let mut result = 0u32;
    let mut shift = 0u32;
    loop {
        let mut b = [0u8; 1];
        match r.read(&mut b) {
            Ok(0) => return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "EOF")),
            Ok(_) => {},
            Err(e) => return Err(e),
        }
        let byte = b[0];
        result |= ((byte & 0x7F) as u32) << shift;
        if (byte & 0x80) == 0 { break; }
        shift += 7;
    }
    Ok(result)
}

//
// ──────────────────────────────────────────────────────────────
//   BITDROP V2 — FULL VERSION WITH:
//   - Skimming
//   - Tagging
//   - Hybrid selector (Zlib small / BD3D large)
//   - Chunking support (serial + parallel)
//   - Auto calibration
//   - Thread‑local BD3D engine reuse
// ─────────────────────────────────────────────────────────────
//

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BitDropV2 {
    pub small_threshold: usize,
    pub skim_enabled: bool,
    pub skim_min_run: usize,
    pub hybrid_threshold: usize,
}

impl Default for BitDropV2 {
    fn default() -> Self {
        Self {
            small_threshold: 512,
            skim_enabled: true,
            skim_min_run: 8,
            hybrid_threshold: 8 * 1024,
        }
    }
}

impl BitDropV2 {
    #[inline]
    fn make_engine() -> BitDrop3DEngine {
        BitDrop3DEngine::new((4, 4, 64), 4)
    }

    thread_local! {
        static ENGINE: RefCell<BitDrop3DEngine> = RefCell::new(BitDropV2::make_engine());
        static TEMP_BUF: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(8192));
    }

    // ──────────────────────────────────────────────────────────────
    // Skimming (RLE-like reversible transform)
    // ──────────────────────────────────────────────────────────────

    fn skim_encode(&self, input: &[u8], out: &mut Vec<u8>) {
        out.clear();
        if input.is_empty() { return; }

        let esc = 0xFF;
        let mut i = 0;
        let n = input.len();

        while i < n {
            let b = input[i];
            let mut j = i + 1;
            while j < n && input[j] == b && (j - i) < 0xFFFF {
                j += 1;
            }
            let run_len = j - i;

            if run_len >= self.skim_min_run {
                out.push(esc);
                out.push(b);
                out.write_u16::<LittleEndian>(run_len as u16).ok();
                i = j;
            } else {
                if b == esc {
                    out.push(esc);
                    out.push(esc);
                    out.write_u16::<LittleEndian>(0).ok();
                } else {
                    out.push(b);
                }
                i += 1;
            }
        }
    }

    fn skim_decode(&self, input: &[u8], out: &mut Vec<u8>) -> Result<(), &'static str> {
        out.clear();
        if input.is_empty() { return Ok(()); }

        let esc = 0xFF;
        let mut i = 0;
        let n = input.len();

        while i < n {
            let b = input[i];
            if b == esc {
                if i + 3 >= n { return Err("skim decode truncated"); }
                let val = input[i + 1];
                let lo = input[i + 2] as u16;
                let hi = input[i + 3] as u16;
                let len = (hi << 8) | lo;

                if len == 0 {
                    out.push(esc);
                } else {
                    for _ in 0..len {
                        out.push(val);
                    }
                }
                i += 4;
            } else {
                out.push(b);
                i += 1;
            }
        }
        Ok(())
    }

    // ──────────────────────────────────────────────────────────────
    // Hybrid selector (Zlib small / BD3D large)
    // ──────────────────────────────────────────────────────────────

    pub fn compress_hybrid(&self, input: &[u8]) -> Vec<u8> {
        if input.is_empty() { return Vec::new(); }

        let mut out = Vec::new();

        if input.len() < self.hybrid_threshold {
            self.compress_into_reuse(input, &mut out);
            return out;
        }

        self.compress_into_reuse(input, &mut out);
        out
    }

    pub fn decompress_hybrid(&self, input: &[u8]) -> Vec<u8> {
        if input.is_empty() { return Vec::new(); }
        let mut out = Vec::new();
        let _ = self.decompress_into_reuse(input, &mut out);
        out
    }

    // ──────────────────────────────────────────────────────────────
    // Core compress/decompress with tag + skimming
    // ──────────────────────────────────────────────────────────────

    pub fn compress_into_reuse(&self, input: &[u8], out: &mut Vec<u8>) {
        out.clear();
        if input.is_empty() { return; }

        let use_zlib = input.len() <= self.small_threshold;

        let mut transformed = Vec::new();
        let mut payload = input;
        let mut skim_flag = false;

        if self.skim_enabled && input.len() >= self.skim_min_run {
            self.skim_encode(input, &mut transformed);
            if transformed.len() < input.len() {
                payload = &transformed;
                skim_flag = true;
            }
        }

        if use_zlib {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
            encoder.write_all(payload).ok();
            let compressed = encoder.finish().unwrap_or_else(|_| payload.to_vec());
            let tag = 0x01 | if skim_flag { 0x80 } else { 0 };
            out.push(tag);
            out.extend_from_slice(&compressed);
            return;
        }

        Self::ENGINE.with(|engine_cell| {
            Self::TEMP_BUF.with(|tmp_cell| {
                let engine = engine_cell.borrow_mut();
                let mut tmp = tmp_cell.borrow_mut();
                tmp.clear();
                let encoded = engine.encode(payload);
                tmp.extend_from_slice(&encoded);

                let tag = 0x02 | if skim_flag { 0x80 } else { 0 };
                out.push(tag);
                out.extend_from_slice(&tmp);
            });
        });
    }

    pub fn decompress_into_reuse(&self, input: &[u8], out: &mut Vec<u8>) -> Result<(), &'static str> {
        out.clear();
        if input.is_empty() { return Ok(()); }

        let tag = input[0];
        let skim_flag = (tag & 0x80) != 0;
        let backend = tag & 0x7F;
        let payload = &input[1..];

        let mut decoded = Vec::new();

        match backend {
            0x01 => {
                let mut decoder = ZlibDecoder::new(payload);
                decoder.read_to_end(&mut decoded).map_err(|_| "zlib decode failed")?;
            }
            0x02 => {
                Self::ENGINE.with(|engine_cell| {
                    let engine = engine_cell.borrow_mut();
                    let d = engine.decode(payload);
                    decoded.extend_from_slice(&d);
                });
            }
            _ => {
                let mut try_buf = Vec::new();
                if ZlibDecoder::new(payload).read_to_end(&mut try_buf).is_ok() {
                    decoded = try_buf;
                } else {
                    Self::ENGINE.with(|engine_cell| {
                        let engine = engine_cell.borrow_mut();
                        let d = engine.decode(payload);
                        decoded.extend_from_slice(&d);
                    });
                }
            }
        }

        if skim_flag {
            self.skim_decode(&decoded, out)?;
        } else {
            out.extend_from_slice(&decoded);
        }

        Ok(())
    }

    // ──────────────────────────────────────────────────────────────
    // Chunking support (serial + parallel)
// ──────────────────────────────────────────────────────────────

    pub fn compress_chunked(&self, input: &[u8], chunk_size: usize) -> Vec<u8> {
        if input.is_empty() { return Vec::new(); }

        let mut out = Vec::new();
        let mut idx: u32 = 0;
        let mut off = 0usize;

        let mut compressed_buf = Vec::new();

        while off < input.len() {
            let end = (off + chunk_size).min(input.len());
            let slice = &input[off..end];

            compressed_buf.clear();
            self.compress_into(slice, &mut compressed_buf);

            out.extend_from_slice(b"BDCH");
            let tag = compressed_buf[0];
            out.push(tag);
            out.extend_from_slice(&(slice.len() as u32).to_le_bytes());
            out.extend_from_slice(&(compressed_buf.len() as u32).to_le_bytes());
            out.extend_from_slice(&idx.to_le_bytes());
            out.extend_from_slice(&compressed_buf);

            off = end;
            idx += 1;
        }

        out
    }

    /// Parallel chunk compression WITHOUT rayon.
    /// Uses std::thread + channels so no external dependencies are required.
    pub fn compress_chunked_parallel(&self, input: &[u8], chunk_size: usize) -> Vec<u8> {
        if input.is_empty() {
            return Vec::new();
        }

        use std::thread;
        use std::sync::mpsc;

        // Build chunk slices
        let mut slices = Vec::new();
        let mut off = 0usize;
        let mut idx = 0u32;

        while off < input.len() {
            let end = (off + chunk_size).min(input.len());
            slices.push((idx, off, end));
            off = end;
            idx += 1;
        }

        let (tx, rx) = mpsc::channel();

        for (chunk_index, start, end) in slices {
            let tx = tx.clone();
            let slice = input[start..end].to_vec();
            let compressor = self.clone();

            thread::spawn(move || {
                let mut buf = Vec::new();
                compressor.compress_into(&slice, &mut buf);
                tx.send((chunk_index, slice.len(), buf)).unwrap();
            });
        }

        drop(tx);

        let mut results = Vec::new();
        for msg in rx {
            results.push(msg);
        }

        results.sort_by_key(|(idx, _, _)| *idx);

        let mut out = Vec::new();
        for (idx, uncompressed_len, compressed_buf) in results {
            out.extend_from_slice(b"BDCH");
            let tag = compressed_buf[0];
            out.push(tag);
            out.extend_from_slice(&(uncompressed_len as u32).to_le_bytes());
            out.extend_from_slice(&(compressed_buf.len() as u32).to_le_bytes());
            out.extend_from_slice(&idx.to_le_bytes());
            out.extend_from_slice(&compressed_buf);
        }

        out
    }

    pub fn decompress_chunked(&self, input: &[u8]) -> Result<Vec<u8>, &'static str> {
        if input.is_empty() { return Ok(Vec::new()); }

        let mut out = Vec::new();
        let mut pos = 0usize;

        while pos < input.len() {
            if pos + 17 > input.len() {
                return Err("chunk header truncated");
            }

            if &input[pos..pos + 4] != b"BDCH" {
                return Err("invalid chunk magic");
            }
            pos += 4;

            let _tag = input[pos];
            pos += 1;

            let uncompressed_len = u32::from_le_bytes(input[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;

            let compressed_len = u32::from_le_bytes(input[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;

            let _chunk_index = u32::from_le_bytes(input[pos..pos + 4].try_into().unwrap());
            pos += 4;

            if pos + compressed_len > input.len() {
                return Err("chunk payload truncated");
            }

            let compressed_payload = &input[pos..pos + compressed_len];
            pos += compressed_len;

            let mut decompressed = Vec::with_capacity(uncompressed_len);
            self.decompress_into(compressed_payload, &mut decompressed);

            if decompressed.len() != uncompressed_len {
                return Err("chunk length mismatch");
            }

            out.extend_from_slice(&decompressed);
        }

        Ok(out)
    }

    // ──────────────────────────────────────────────────────────────
    // Auto calibration
    // ──────────────────────────────────────────────────────────────

    /// Auto-calibrate hybrid threshold + small threshold + best chunk size.
    pub fn auto_calibrate(&mut self, sample: &[u8]) -> usize {
        if sample.is_empty() {
            return self.hybrid_threshold;
        }

        let candidate_chunk_sizes = [1024usize, 2048, 4096, 8192, 16384];

        let mut best_chunk_size = candidate_chunk_sizes[0];
        let mut best_bytes = usize::MAX;
        let mut best_ms = f64::MAX;

        for &chunk_size in &candidate_chunk_sizes {
            let start = Instant::now();
            let compressed = self.compress_chunked_parallel(sample, chunk_size);
            let elapsed = start.elapsed();
            let ms = elapsed.as_secs_f64() * 1000.0;
            let bytes = compressed.len();

            let better =
                bytes < best_bytes ||
                (bytes == best_bytes && ms < best_ms);

            if better {
                best_bytes = bytes;
                best_ms = ms;
                best_chunk_size = chunk_size;
            }
        }

        self.hybrid_threshold = best_chunk_size;
        self.small_threshold = best_chunk_size / 2;

        best_chunk_size
    }
}

impl BitDropCompressor for BitDropV2 {
    fn compress(&self, input: &[u8]) -> Vec<u8> {
        self.compress_hybrid(input)
    }

    fn decompress(&self, input: &[u8]) -> Vec<u8> {
        self.decompress_hybrid(input)
    }

    fn compress_into(&self, input: &[u8], out: &mut Vec<u8>) {
        let hybrid = self.compress_hybrid(input);
        out.clear();
        out.extend_from_slice(&hybrid);
    }

    fn decompress_into(&self, input: &[u8], out: &mut Vec<u8>) {
        let hybrid = self.decompress_hybrid(input);
        out.clear();
        out.extend_from_slice(&hybrid);
    }
}


