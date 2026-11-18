//! Generates `scene.h` for `gpui-pre-apple` runtime shaders on non-macOS hosts.
//!
//! The crate's own build script skips shader work when it runs on Linux, which
//! is exactly what happens during a cross build. This helper reproduces the
//! header generation it would have done, using the same config and source
//! files. Without the header the vendored shader compiles to nothing at runtime
//! and the app fails with "building Metal library".

use std::path::PathBuf;

use cbindgen::Config;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(gpui_dir), Some(apple_src), Some(out_path)) = (args.next(), args.next(), args.next())
    else {
        eprintln!("usage: shader-header <gpui-dir> <apple-src> <out-header>");
        std::process::exit(2);
    };
    let gpui_dir = PathBuf::from(gpui_dir);
    let apple_src = PathBuf::from(apple_src);
    let out_path = PathBuf::from(out_path);

    let mut config = Config {
        include_guard: Some("SCENE_H".into()),
        language: cbindgen::Language::C,
        no_includes: true,
        ..Default::default()
    };
    config.export.include.extend([
        "Bounds".to_string(),
        "Corners".into(),
        "Edges".into(),
        "Size".into(),
        "Pixels".into(),
        "PointF".into(),
        "Hsla".into(),
        "ContentMask".into(),
        "Uniforms".into(),
        "AtlasTile".into(),
        "PathRasterizationInputIndex".into(),
        "PathVertex_ScaledPixels".into(),
        "PathRasterizationVertex".into(),
        "ShadowInputIndex".into(),
        "Shadow".into(),
        "QuadInputIndex".into(),
        "Underline".into(),
        "UnderlineInputIndex".into(),
        "Quad".into(),
        "BorderStyle".into(),
        "SpriteInputIndex".into(),
        "MonochromeSprite".into(),
        "PolychromeSprite".into(),
        "PathSprite".into(),
        "SurfaceInputIndex".into(),
        "SurfaceBounds".into(),
        "TransformationMatrix".into(),
    ]);
    config.enumeration.prefix_with_name = true;

    let mut builder = cbindgen::Builder::new();
    let sources = [
        gpui_dir.join("src/scene.rs"),
        gpui_dir.join("src/geometry.rs"),
        gpui_dir.join("src/color.rs"),
        gpui_dir.join("src/window.rs"),
        gpui_dir.join("src/platform.rs"),
        apple_src.join("src/metal_renderer.rs"),
    ];
    for source in &sources {
        if !source.is_file() {
            eprintln!("error: missing shader source {}", source.display());
            std::process::exit(1);
        }
        builder = builder.with_src(source);
    }

    builder
        .with_config(config)
        .generate()
        .expect("cbindgen could not build the shader header")
        .write_to_file(&out_path);

    let written = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
    if written == 0 {
        eprintln!("error: wrote an empty {}", out_path.display());
        std::process::exit(1);
    }
    println!("wrote {} bytes to {}", written, out_path.display());
}
