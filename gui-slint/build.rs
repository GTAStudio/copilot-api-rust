use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::{Read, Write};

fn main() {
    println!("cargo::rustc-check-cfg=cfg(embedded_server)");
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent".into())
        .with_debug_info(std::env::var("PROFILE").as_deref() == Ok("debug"))
        .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles);
    slint_build::compile_with_config("ui/app.slint", config).expect("Failed to compile Slint UI");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_icon().expect("Failed to embed Windows application icon");
    }

    println!("cargo:rerun-if-env-changed=COPILOT_SERVER_PATH");
    let executable = if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        "copilot-api-server.exe"
    } else {
        "copilot-api-server"
    };
    let default_path =
        std::path::PathBuf::from(format!("../rust-server/target/release/{executable}"));
    println!("cargo:rerun-if-changed={}", default_path.display());
    let explicit = std::env::var_os("COPILOT_SERVER_PATH").map(std::path::PathBuf::from);
    let server_path = explicit.or_else(|| default_path.is_file().then_some(default_path));
    let compressed_path = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"))
        .join("server_embedded.gz");

    if let Some(server_path) = server_path {
        println!("cargo:rerun-if-changed={}", server_path.display());

        let mut input = std::fs::File::open(server_path).expect("Cannot open server exe");
        let mut data = Vec::new();
        input
            .read_to_end(&mut data)
            .expect("Cannot read server exe");

        let output = std::fs::File::create(compressed_path).expect("Cannot create compressed file");
        let mut encoder = GzEncoder::new(output, Compression::best());
        encoder.write_all(&data).expect("Cannot compress");
        encoder.finish().expect("Cannot finish compression");

        println!("cargo:rustc-cfg=embedded_server");
    } else {
        assert_ne!(
            std::env::var("PROFILE").as_deref(),
            Ok("release"),
            "Build rust-server --release first or set COPILOT_SERVER_PATH"
        );
        println!("cargo:warning=No server executable found. Build rust-server or copilot-api-server.exe first.");
    }
}

fn embed_windows_icon() -> Result<(), Box<dyn std::error::Error>> {
    use image::codecs::ico::{IcoEncoder, IcoFrame};

    println!("cargo:rerun-if-changed=ui/assets/app-icon.png");
    let source = image::open("ui/assets/app-icon.png")?;
    let frames = [16, 20, 24, 32, 48, 64, 128, 256]
        .into_iter()
        .map(|size| {
            let pixels = source
                .resize_exact(size, size, image::imageops::FilterType::Lanczos3)
                .into_rgba8();
            IcoFrame::as_png(pixels.as_raw(), size, size, image::ExtendedColorType::Rgba8)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let icon_path = std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("Missing OUT_DIR")?)
        .join("app-icon.ico");
    IcoEncoder::new(std::fs::File::create(&icon_path)?).encode_images(&frames)?;
    winresource::WindowsResource::new()
        .set_icon(icon_path.to_str().ok_or("Invalid icon path")?)
        .set("ProductName", "GTA Copilot API")
        .set("FileDescription", "GTA Copilot API")
        .compile()?;
    Ok(())
}
