# kindle-stripper

<img align="right" src="./logo.svg" alt="Kindle-stripper mascot twirling an open padlock" width="180">

Kindle-stripper is a standalone Windows command-line tool I built to simplify removing DRM from Kindle books I already own and converting them for personal use. Run it with no arguments to list downloaded ASINs and titles. Exports default to KFX-ZIP. Add `--convert` for EPUB 3 or PDF for supported PDF-backed print replicas.

<br clear="all">

## Documentation

- [Setup](docs/setup.md) covers the Windows release, Kindle downloads, and building from source.
- [Usage](docs/usage.md) covers export formats, PowerShell examples, batch behaviour, and troubleshooting.
- [How it works](docs/implementation.md) traces a download through the Rust pipeline and explains the design choices and validation limits.

## Kindle for Windows requirements

Install Kindle for Windows, sign in to your Amazon account in the app, and download the books you want to export. Run Kindle-stripper under the same Windows user account that downloaded the books.

This project was built and tested against **Kindle for Windows 1.1.5634.0**. Compatibility with newer versions is not guaranteed.

## Personal use only

Use Kindle-stripper only for books you already have access to through your own Kindle account. Keep exported books for your personal use. Never share or distribute them.

## Build

Use the Windows MSVC Rust toolchain with the Visual Studio C++ build tools. The release build was verified on Rust 1.100.0.

```powershell
cargo test --locked --release
cargo build --locked --release
```

The executable is at `target\release\kindle-stripper.exe`. It runs without Calibre, Python, or a separate converter.

## Use

```powershell
.\kindle-stripper.exe
.\kindle-stripper.exe B008E8XV56
.\kindle-stripper.exe "B008E8XV56,B00XLII45G" --convert --output-folder "D:\Kindle Exports"
.\kindle-stripper.exe --all --convert --output-folder "D:\Kindle Exports"
```

The output folder must already exist. Default filenames include the title, first author, and ASIN when available. Use `--output` to name a single export explicitly or `--quiet` to suppress progress and success messages. Batch exports preserve existing files, continue after individual failures, and exit with a nonzero status if any book failed.

Boko is the default EPUB engine. Use `--epub-engine basic` to select the built-in exporter. EPUB layout and navigation fidelity vary by book. Run `--help` for the full command syntax.

## Attribution and license

The original concepts and reverse engineering came from upstream developers. My contribution was to rework their approach into a simpler Rust tool. Kindle-stripper is based on [Satsuoni's fork of DeDRM Tools](https://github.com/Satsuoni/DeDRM_tools/). John Howell's [KFX Input plugin](https://www.mobileread.com/forums/showthread.php?t=291290) served as a KFX format reference. The default EPUB engine uses [boko](https://github.com/zacharydenton/boko).

This project is licensed under GPL-3.0-only. See [LICENSE](LICENSE).
