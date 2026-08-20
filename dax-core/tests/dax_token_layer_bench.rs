use dax_core::token_layer::{
    base_tokenizer::StubTokenizer,
    delta_codec::SimpleDaxDeltaCodec,
    bitdrop_adapter::StubBitDrop,
    DaxTokenLayer,
};

use std::time::Instant;

fn main() {
    let tokenizer = StubTokenizer;
    let delta_codec = SimpleDaxDeltaCodec;
    let compressor = StubBitDrop;

    let mut layer = DaxTokenLayer::new(tokenizer, delta_codec, compressor);

    let text_samples = vec![
        "Hello world, this is a DAX token layer benchmark.",
        "The quick brown fox jumps over the lazy dog.",
        "SyntheticMind is building a unified cognitive engine.",
        "Delta compression reduces KV-cache load dramatically.",
        "This is a longer sample text to simulate real workloads and stress test the delta layer.",
    ];

    println!("=== DAX Token Layer Benchmark ===");

    for text in text_samples {
        println!("\n--- Sample ---");
        println!("Text: {}", text);

        // Normal tokenization
        let t0 = Instant::now();
        let normal_tokens = layer.encode_only(text);
        let normal_time = t0.elapsed();

        // DAX delta encode
        let t1 = Instant::now();
        let (dax_tokens, compressed_delta) = layer.encode_with_delta(text);
        let dax_time = t1.elapsed();

        // Reconstruction
        let t2 = Instant::now();
        let reconstructed = layer.reconstruct_tokens(&normal_tokens, &compressed_delta);
        let reconstruct_time = t2.elapsed();

        // Validate correctness
        let ok = reconstructed == dax_tokens;

        println!("Normal tokenization:   {:?}", normal_time);
        println!("DAX encode + compress: {:?}", dax_time);
        println!("Reconstruct:           {:?}", reconstruct_time);
        println!("Correct reconstruction: {}", ok);

        let ratio = dax_time.as_nanos() as f64 / normal_time.as_nanos() as f64;
        println!("Speed ratio (DAX / normal): {:.3}", ratio);
    }

    println!("\nBenchmark complete.");
}
