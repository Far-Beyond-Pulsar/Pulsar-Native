# Set Logo Tool

A command-line tool to easily set the Pulsar application logo across all platforms (Windows, macOS, and Linux).

## Usage

From the Pulsar root directory:

```bash
cargo set-logo path/to/your/logo.png
```

This will automatically generate:
- **Windows**: `logo_sqrkl.ico` (embedded in the executable)
- **macOS**: `logo_sqrkl.icns` (for app bundles)
- **Linux**: `logo_sqrkl.png` + `pulsar.desktop` file

## Options

```bash
# Dry run - see what would happen without making changes
cargo set-logo --dry-run path/to/logo.png

# Custom output directory
cargo set-logo -o custom/path path/to/logo.png

# Help
cargo set-logo --help
```

## Requirements

- Input should be a PNG file
- Square images work best (recommended: 512x512 or 1024x1024)
- Supports transparent backgrounds

## What It Does

1. **Validates** your input PNG
2. **Generates multiple formats**:
   - ICO with 16x16, 32x32, 48x48, 256x256 resolutions
   - ICNS with 16x16, 32x32, 128x128, 256x256, 512x512, 1024x1024 resolutions
   - PNG at 256x256
3. **Creates** a Linux .desktop entry file
4. **Saves** everything to `assets/images/` (or custom path)

## After Running

1. **Rebuild** your project: `cargo build`
2. **Windows**: Icon will be automatically embedded in the `.exe`
3. **macOS**: Copy the `.icns` file to your app bundle's `Resources` folder
4. **Linux**: Install the `.desktop` file to `~/.local/share/applications/`

## Examples

```bash
# Use the existing Pulsar logo
cargo set-logo assets/images/logo_round.png

# Use a new custom logo
cargo set-logo ~/Downloads/my-awesome-logo.png

# Test without making changes
cargo set-logo --dry-run new-logo.png
```
