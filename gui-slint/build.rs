use std::io::{Read, Write};
use flate2::write::GzEncoder;
use flate2::Compression;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(embedded_server)");
    let config = slint_build::CompilerConfiguration::new().with_style("material".into());
    slint_build::compile_with_config("ui/app.slint", config)
        .expect("Failed to compile Slint UI");
    
    // Compress server exe for embedding
    let server_candidates = [
        "../copilot-api-server.exe",
        "../rust-server/target/release/copilot-api-server.exe",
    ];
    let compressed_path = "src/server_embedded.gz";

    let server_path = server_candidates
        .iter()
        .find(|path| std::path::Path::new(path).exists())
        .copied();

    if let Some(server_path) = server_path {
        println!("cargo:rerun-if-changed={}", server_path);

        let mut input = std::fs::File::open(server_path).expect("Cannot open server exe");
        let mut data = Vec::new();
        input.read_to_end(&mut data).expect("Cannot read server exe");
        
        let output = std::fs::File::create(compressed_path).expect("Cannot create compressed file");
        let mut encoder = GzEncoder::new(output, Compression::best());
        encoder.write_all(&data).expect("Cannot compress");
        encoder.finish().expect("Cannot finish compression");
        
        println!("cargo:rustc-cfg=embedded_server");
    } else {
        println!("cargo:warning=No server executable found. Build rust-server or copilot-api-server.exe first.");
    }
}
