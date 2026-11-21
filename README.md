# kindle-stripper

A standalone Windows command-line tool for exporting locally downloaded Kindle KFX books. Run it with no arguments to list downloaded ASINs and titles. Exports default to KFX-ZIP. Add `--convert` for EPUB 3 or PDF for supported PDF-backed print replicas.

## Build

Use the Windows MSVC Rust toolchain with the Visual Studio C++ build tools. The release build was verified on Rust 1.100.0.

```powershell
cargo test --locked --release
cargo build --locked --release
```

The executable is at `target\release\kindle-stripper.exe`. It runs without Calibre, Python, or a separate converter. For protected books, run it under the Windows account that downloaded them in the Kindle app.

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

Kindle-stripper is a Rust port based on [Satsuoni's fork of DeDRM Tools](https://github.com/Satsuoni/DeDRM_tools/). John Howell's [KFX Input plugin](https://www.mobileread.com/forums/showthread.php?t=291290) served as a KFX format reference. The default EPUB engine uses [boko](https://github.com/zacharydenton/boko).

This project is licensed under GPL-3.0-only. See [LICENSE](LICENSE).
