use anyhow::{Context, Result, bail};
use clap::Parser;
use image::{ImageFormat, GenericImageView, ImageReader};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(name = "set-logo")]
#[command(about = "Set Pulsar application logo across all platforms", long_about = None)]
struct Args {
    /// Path to the PNG logo file
    #[arg(value_name = "PNG_FILE")]
    input: PathBuf,

    /// Dry run - show what would be done without making changes
    #[arg(long)]
    dry_run: bool,

    /// Output directory for generated files (default: assets/images)
    #[arg(short, long)]
    output: Option<PathBuf>,
}

const SIZES: &[u32] = &[16, 32, 48, 64, 128, 256, 512, 1024, 2048, 4096];

fn main() -> Result<()> {
    let args = Args::parse();

    println!("🎨 Pulsar Logo Tool");
    println!("==================\n");

    // Validate input PNG
    println!("📂 Loading PNG: {}", args.input.display());
    if !args.input.exists() {
        bail!("Input file does not exist: {}", args.input.display());
    }

    let img = ImageReader::open(&args.input)
        .context("Failed to open image")?
        .decode()
        .context("Failed to decode image")?;

    let (width, height) = img.dimensions();
    println!("   ✓ Dimensions: {}x{}", width, height);

    if width != height {
        println!("   ⚠️  Warning: Image is not square ({}x{})", width, height);
        println!("      This may cause distortion on some platforms.");
    }

    // Determine output directory
    let output_dir = match &args.output {
        Some(path) => path.clone(),
        None => {
            let mut path = std::env::current_dir()?;
            path.push("assets");
            path.push("images");
            path
        }
    };

    if args.dry_run {
        println!("\n🔍 DRY RUN MODE - No files will be modified\n");
    } else {
        fs::create_dir_all(&output_dir)
            .context("Failed to create output directory")?;
    }

    println!("\n📦 Output directory: {}", output_dir.display());

    // Generate ICO for Windows
    println!("\n🪟 Windows (.ico):");
    generate_ico(&img, &output_dir, args.dry_run)?;

    // Generate ICNS for macOS
    println!("\n🍎 macOS (.icns):");
    generate_icns(&img, &output_dir, args.dry_run)?;

    // Generate PNG copies
    println!("\n🐧 Linux (PNG):");
    generate_png_copies(&img, &output_dir, args.dry_run)?;

    // Generate .desktop file template
    println!("\n📄 Linux Desktop Entry:");
    generate_desktop_file(&output_dir, args.dry_run)?;

    if !args.dry_run {
        println!("\n✅ Logo files generated successfully!");
        println!("\n📋 Next steps:");
        println!("   1. Rebuild your project: cargo build");
        println!("   2. On Windows: Icon will be embedded in the .exe");
        println!("   3. On macOS: Use the .icns file in your app bundle");
        println!("   4. On Linux: Install the .desktop file to ~/.local/share/applications/");
    } else {
        println!("\n✅ Dry run complete - no files were modified");
    }

    Ok(())
}

fn generate_ico(img: &image::DynamicImage, output_dir: &Path, dry_run: bool) -> Result<()> {
    let ico_path = output_dir.join("logo_sqrkl.ico");
    println!("   → {}", ico_path.display());

    if dry_run {
        println!("      (would generate with sizes: 16, 32, 48, 256)");
        return Ok(());
    }

    let mut ico_dir = ico::IconDir::new(ico::ResourceType::Icon);

    // Generate key sizes for ICO (Windows standard)
    for &size in &[16u32, 32, 48, 256] {
        let resized = img.resize_exact(size, size, image::imageops::FilterType::Lanczos3);
        let rgba = resized.to_rgba8();
        
        let ico_image = ico::IconImage::from_rgba_data(size, size, rgba.into_raw());
        ico_dir.add_entry(ico::IconDirEntry::encode(&ico_image)?);
        println!("      ✓ {}x{}", size, size);
    }

    let file = File::create(&ico_path)
        .context("Failed to create ICO file")?;
    ico_dir.write(file)
        .context("Failed to write ICO file")?;

    Ok(())
}

fn generate_icns(img: &image::DynamicImage, output_dir: &Path, dry_run: bool) -> Result<()> {
    let icns_path = output_dir.join("logo_sqrkl.icns");
    println!("   → {}", icns_path.display());

    if dry_run {
        println!("      (would generate with sizes: 16, 32, 128, 256, 512)");
        return Ok(());
    }

    let mut icon_family = icns::IconFamily::new();

    // macOS icon sizes - icns crate auto-detects icon type from dimensions
    let mac_sizes = [16u32, 32, 128, 256, 512];

    for size in mac_sizes.iter() {
        let resized = img.resize_exact(*size, *size, image::imageops::FilterType::Lanczos3);
        let rgba = resized.to_rgba8();
        
        let icon_image = icns::Image::from_data(
            icns::PixelFormat::RGBA,
            *size,
            *size,
            rgba.into_raw()
        )?;

        icon_family.add_icon(&icon_image)?;
        println!("      ✓ {}x{}", size, size);
    }

    let file = File::create(&icns_path)
        .context("Failed to create ICNS file")?;
    icon_family.write(file)
        .context("Failed to write ICNS file")?;

    Ok(())
}

fn generate_png_copies(img: &image::DynamicImage, output_dir: &Path, dry_run: bool) -> Result<()> {
    // Generate standard sizes
    let png_path = output_dir.join("logo_sqrkl.png");
    println!("   → {}", png_path.display());

    if dry_run {
        println!("      (would generate: logo_sqrkl.png at 256x256)");
        return Ok(());
    }

    let resized = img.resize_exact(256, 256, image::imageops::FilterType::Lanczos3);
    resized.save_with_format(&png_path, ImageFormat::Png)
        .context("Failed to save PNG")?;
    println!("      ✓ 256x256");

    Ok(())
}

fn generate_desktop_file(output_dir: &Path, dry_run: bool) -> Result<()> {
    let desktop_path = output_dir.join("pulsar.desktop");
    println!("   → {}", desktop_path.display());

    if dry_run {
        println!("      (would generate desktop entry file)");
        return Ok(());
    }

    let desktop_content = r#"[Desktop Entry]
Type=Application
Name=Pulsar Engine
Comment=A modern high-performance game engine
Exec=/usr/local/bin/pulsar_engine
Icon=/usr/share/pixmaps/pulsar.png
Terminal=false
Categories=Development;IDE;
Keywords=game;engine;development;
"#;

    let mut file = BufWriter::new(File::create(&desktop_path)?);
    file.write_all(desktop_content.as_bytes())?;
    file.flush()?;
    
    println!("      ✓ Desktop entry created");
    println!("      Note: You may need to adjust paths in the .desktop file");

    Ok(())
}
