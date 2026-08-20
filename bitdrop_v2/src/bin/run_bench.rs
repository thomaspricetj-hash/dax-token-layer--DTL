use bitdrop_v2::{
    compress,
    decompress,
    gpu_available,
    init_gpu_backend,
    bitdrop_encode_dax_auto,
    bitdrop_decode_dax_auto,
};

use std::time::Instant;
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;

// ---------------------------------------------------------
// Cube extractor (4×4×64 cubes)
// ---------------------------------------------------------
fn extract_cubes(data: &[u8]) -> Vec<Vec<u8>> {
    let cube_size = 4 * 4 * 64; // 1024 bytes
    data.chunks(cube_size).map(|c| c.to_vec()).collect()
}

// ---------------------------------------------------------
// Token extractor (simple 32-bit tokens)
// ---------------------------------------------------------
fn extract_tokens(data: &[u8]) -> Vec<u32> {
    let mut out = Vec::new();
    for chunk in data.chunks(4) {
        let mut buf = [0u8; 4];
        for i in 0..chunk.len() {
            buf[i] = chunk[i];
        }
        out.push(u32::from_be_bytes(buf));
    }
    out
}

// ---------------------------------------------------------
// Standard BitDrop v2 benchmark (raw payload → BD3D)
// ---------------------------------------------------------
fn bench_case(name: &str, data: &[u8]) {
    println!("\n=== {} ===", name);

    let start = Instant::now();
    let out = compress(data);
    let t_comp = start.elapsed();

    let start = Instant::now();
    let back = decompress(&out);
    let t_decomp = start.elapsed();

    let ok = back == data;

    println!("input:   {} bytes", data.len());
    println!("output:  {} bytes", out.len());
    println!("ratio:   {:.6}", out.len() as f64 / data.len() as f64);
    println!("correct: {}", ok);
    println!("compress: {:?}", t_comp);
    println!("decomp:   {:?}", t_decomp);
}

// ---------------------------------------------------------
// Hybrid DAX benchmark (raw + cubes + tokens)
// ---------------------------------------------------------
fn bench_case_dax_hybrid(name: &str, data: &[u8], token_count: usize) {
    println!("\n=== {} (Hybrid DAX) ===", name);

    // Build raw shards
    let mut raw_shards = Vec::new();
    raw_shards.push(("master".to_string(), data.to_vec()));

    for i in 1..token_count {
        let mut t = data.to_vec();
        let idx = (i * 97) % t.len();
        t[idx] ^= i as u8;
        raw_shards.push((format!("token{}", i), t));
    }

    // Build cube shards
    let mut cube_shards = Vec::new();
    for (name, d) in raw_shards.iter() {
        cube_shards.push((name.clone(), extract_cubes(d)));
    }

    // Build token shards
    let mut token_shards = Vec::new();
    for (name, d) in raw_shards.iter() {
        token_shards.push((name.clone(), extract_tokens(d)));
    }

    let total_input: usize = raw_shards.iter().map(|(_, d)| d.len()).sum();

    let start = Instant::now();
    let out = bitdrop_encode_dax_auto(raw_shards.clone(), Some(cube_shards), Some(token_shards));
    let t_comp = start.elapsed();

    let start = Instant::now();
    let decoded = bitdrop_decode_dax_auto(&out);
    let t_decomp = start.elapsed();

    let ok = decoded[0].1 == data;

    println!("master size:          {} bytes", data.len());
    println!("total token size:     {} bytes", total_input);
    println!("output(frame):        {} bytes", out.len());
    println!("ratio vs master:      {:.6}", out.len() as f64 / data.len() as f64);
    println!("ratio vs total:       {:.6}", out.len() as f64 / total_input as f64);
    println!("correct(master): {}", ok);
    println!("compress: {:?}", t_comp);
    println!("decomp:   {:?}", t_decomp);
}

// ---------------------------------------------------------
// Payload generators
// ---------------------------------------------------------
fn random_bytes(n: usize) -> Vec<u8> {
    let mut rng = StdRng::seed_from_u64(12345);
    (0..n).map(|_| rng.gen::<u8>()).collect()
}

fn repetitive_bytes(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 4) as u8).collect()
}

fn structured_payload_40mb() -> Vec<u8> {
    let target = 40 * 1024 * 1024;
    let mut v = Vec::with_capacity(target);
    let mut i: u32 = 0;

    while v.len() < target {
        v.extend_from_slice(&i.to_le_bytes());
        i = i.wrapping_add(1);
    }

    v
}

// ---------------------------------------------------------
// Main
// ---------------------------------------------------------
fn main() {
    println!("BitDrop v3 Hybrid Benchmark");
    println!("GPU available: {}", gpu_available());

    init_gpu_backend();

    // Standard BitDrop v2
    bench_case("small text", b"hello world, this is a test payload");
    bench_case("random 64 KB", &random_bytes(64 * 1024));
    bench_case("random 40 MB", &random_bytes(40 * 1024 * 1024));
    bench_case("repetitive 40 MB", &repetitive_bytes(40 * 1024 * 1024));
    bench_case("structured 40 MB", &structured_payload_40mb());

    // Hybrid DAX (raw + cubes + tokens)
    bench_case_dax_hybrid("Hybrid DAX small text", b"hello world, this is a test payload", 3);
    bench_case_dax_hybrid("Hybrid DAX structured 40 MB (3 tokens)", &structured_payload_40mb(), 3);
    bench_case_dax_hybrid("Hybrid DAX structured 40 MB (100 tokens)", &structured_payload_40mb(), 100);
}



