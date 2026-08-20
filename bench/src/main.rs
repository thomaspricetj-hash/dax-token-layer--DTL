use std::time::Instant;

use dax_core::token_layer::{
    base_tokenizer::StubTokenizer,
    delta_codec::{DaxDeltaCodec, DaxMasterDeltaCodec, GroupIndexer, decompress_and_unpack},
    bitdrop_adapter::BitDropV2,
};

use dax_core::token_layer::bitdrop_adapter::BitDropCompressor;

fn main() {
    let tokenizer = StubTokenizer;
    let delta_codec = DaxMasterDeltaCodec;

    let samples = vec![
        "Hello world, this is a DAX token layer benchmark.",
        "The quick brown fox jumps over the lazy dog.",
        "SyntheticMind is building a unified cognitive engine.",
        "Delta compression reduces KV-cache load dramatically.",
        "This is a longer sample text to simulate real workloads and stress test the delta layer.",
    ];

    let group_sizes = [32usize, 64, 128, 256, 512];

    let v2 = BitDropV2::default();

    const WARMUP: usize = 5;
    const RUNS: usize = 10;

    for &group_size in &group_sizes {
        // Expand samples
        let mut expanded = Vec::with_capacity(group_size);
        for i in 0..group_size {
            expanded.push(samples[i % samples.len()].to_string());
        }

        //
        // RAW TOKENIZATION BENCHMARK
        //
        let mut raw_total = std::time::Duration::ZERO;
        let mut raw_bytes = 0usize;

        // Warm-up
        for _ in 0..WARMUP {
            let mut tmp = Vec::new();
            for text in &expanded {
                let tokens = tokenizer.encode(text);
                tmp.extend_from_slice(&tokens);
            }
        }

        // Timed runs
        for _ in 0..RUNS {
            let t = Instant::now();
            let mut tmp = Vec::new();
            for text in &expanded {
                let tokens = tokenizer.encode(text);
                tmp.extend_from_slice(&tokens);
            }
            raw_total += t.elapsed();
            raw_bytes = tmp.len() * 4; // u32 tokens
        }

        let raw_avg_ms = (raw_total.as_secs_f64() * 1000.0) / RUNS as f64;

        //
        // FULL HYBRID BD3D PIPELINE BENCHMARK
        //
        let mut token_streams = Vec::with_capacity(group_size);
        for text in &expanded {
            token_streams.push(tokenizer.encode(text));
        }

        let master = token_streams[0].clone();

        let mut indexer = GroupIndexer::new();
        for t in &token_streams {
            let raw = delta_codec.diff(&master, t);
            indexer.add(raw);
        }

        let grouped_uncompressed = indexer.finalize_uncompressed();

        let mut bd3d_total = std::time::Duration::ZERO;
        let mut last_compressed = Vec::new();

        // Warm-up
        for _ in 0..WARMUP {
            let _ = v2.compress(&grouped_uncompressed);
        }

        // Timed runs
        for _ in 0..RUNS {
            let t = Instant::now();
            let out = v2.compress(&grouped_uncompressed);
            bd3d_total += t.elapsed();
            last_compressed = out;
        }

        let bd3d_avg_ms = (bd3d_total.as_secs_f64() * 1000.0) / RUNS as f64;
        let bd3d_bytes = last_compressed.len();

        //
        // VERIFY CORRECTNESS
        //
        let raws = decompress_and_unpack(&last_compressed, &v2).expect("unpack failed");
        let mut ok = true;

        for i in 0..token_streams.len() {
            let rec = delta_codec.apply(&master, &raws[i]).expect("apply failed");
            if rec != token_streams[i] {
                ok = false;
                break;
            }
        }

        //
        // LABELED, HUMAN‑READABLE OUTPUT
        //
        println!("\n====================================================");
        println!("                GROUP SIZE: {}", group_size);
        println!("====================================================");

        println!("RAW TOKENIZATION (baseline)");
        println!("  • Raw size (bytes): {}", raw_bytes);
        println!("  • Avg tokenization time: {:.3} ms", raw_avg_ms);

        println!("\nMY DESIGN (DAX delta + grouping + BD3D hybrid)");
        println!("  • Compressed size (bytes): {}", bd3d_bytes);
        println!("  • Avg compression time: {:.3} ms", bd3d_avg_ms);
        println!("  • Correct reconstruction: {}", ok);

        println!("\nSUMMARY:");
        println!("  • Size reduction: {:.2}x smaller than raw",
                 raw_bytes as f64 / bd3d_bytes as f64);
        println!("  • Speed comparison: BD3D is {:.2}x {} than raw",
                 if bd3d_avg_ms > 0.0 { raw_avg_ms / bd3d_avg_ms } else { 0.0 },
                 if bd3d_avg_ms < raw_avg_ms { "FASTER" } else { "SLOWER" });

        println!("====================================================\n");
    }
}

