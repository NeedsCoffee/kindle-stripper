# kindle-stripper

<img align="right" src="./logo.svg" alt="Kindle-stripper mascot twirling an open padlock" width="180">

Kindle-stripper is a standalone Windows tool I built to simplify removing DRM from Kindle books I already own and exporting them for personal use. Double-click the executable to choose books, an export format, and where to save them.

<br clear="all">

## Export books interactively

1. Sign in to Kindle for Windows and download the books you want to export.
2. Double-click `kindle-stripper.exe` in File Explorer. Enter one or more book numbers or ASINs from the list, a mix such as `3, B008E8XV56, 1`, a book folder, or `all`.
3. Choose **KFX archive** to keep the KFX components, or **EPUB/PDF** to convert. Reflowable books export as EPUB 3. Supported PDF-backed print replicas export as PDF.
4. Enter an output folder. Kindle-stripper can create it if it is missing. When the export finishes, press Enter to close the window.

The prompts are available from version 0.200.0. See the [setup guide](docs/setup.md) for installation or building from source, and the [interactive walkthrough](docs/usage.md#use-the-interactive-prompts) for each prompt and its defaults.

## Kindle for Windows requirements

Install Kindle for Windows, sign in to your Amazon account in the app, and download the books you want to export. Run Kindle-stripper under the same Windows user account that downloaded the books.

This project was built and tested against **Kindle for Windows 1.1.5634.0**. Compatibility with newer versions is not guaranteed.

## Personal use only

Use Kindle-stripper only for books you already have access to through your own Kindle account. Keep exported books for your personal use. Never share or distribute them.

## Command-line usage

In an existing terminal, run `-i` or `--interactive` to open the same prompts. Run the executable without arguments to list downloaded ASINs and titles. Pass a book selection and options to export directly.

```powershell
.\kindle-stripper.exe
.\kindle-stripper.exe --interactive
.\kindle-stripper.exe B008E8XV56
.\kindle-stripper.exe "B008E8XV56,B00XLII45G" --convert --output-folder "D:\Kindle Exports"
.\kindle-stripper.exe --all --convert --output-folder "D:\Kindle Exports"
```

Direct exports default to KFX-ZIP. Add `--convert` for EPUB 3 or PDF for supported PDF-backed print replicas. When passing an output folder as a command-line argument, use an existing directory. Default filenames include the title, first author, and ASIN when available. Use `--output` to name a single export explicitly or `--quiet` to suppress progress and success messages. Batch exports preserve existing files, continue after individual failures, and exit with a nonzero status if any book failed.

Boko is the default EPUB engine. Use `--epub-engine basic` to select the built-in exporter. EPUB layout and navigation fidelity vary by book. Run `--help` for the full command syntax or see the [PowerShell examples and scripting options](docs/usage.md#command-line-reference).

## Documentation

- [Setup](docs/setup.md) covers the Windows release, Kindle downloads, and building from source.
- [Usage](docs/usage.md) covers the interactive prompts, export formats, troubleshooting, and the command-line reference.
- [How it works](docs/implementation.md) traces a download through the Rust pipeline and explains the design choices and validation limits.
- [Code flow diagrams](docs/code-flow.md) show launch detection, interactive selection, and the export pipeline in Mermaid.

## Build

Use the Windows MSVC Rust toolchain with the Visual Studio C++ build tools and Windows SDK. The release build was verified on Rust 1.100.0.

```powershell
cargo test --locked --release
cargo build --locked --release
```

The executable is at `target\release\kindle-stripper.exe`. The build embeds the mascot icon, with 15 sizes from 16×16 to 256×256 pixels. The executable runs without Calibre, Python, or a separate converter.

## Attribution and license

Kindle-stripper owes its existence to the people who wrote, published, and maintained the original tools. The Dark Reverser published the early Kindle and Mobipocket tools, and i♥cabbages published the early EPUB tools. Apprentice Alf brought the tools together and improved them alongside contributors including DiapDealer and some_updates. Apprentice Harper later took over maintenance and preserved the project's history. [Apprentice Alf's account of the early tools](https://apprenticealf.wordpress.com/author/apprenticealf/) · [Apprentice Harper's DeDRM Tools](https://github.com/apprenticeharper/DeDRM_tools)

noDRM continued DeDRM Tools from Apprentice Harper's repository, and Satsuoni forked noDRM's project. Satsuoni's work on Kindle for Windows is the immediate basis for Kindle-stripper. My contribution was to rework that approach into a simpler Rust tool. Thank you to everyone whose reverse engineering, code, and maintenance made it possible. [noDRM's DeDRM Tools](https://github.com/noDRM/DeDRM_tools) · [Satsuoni's DeDRM Tools](https://github.com/Satsuoni/DeDRM_tools)

John Howell's KFX Input plugin served as a KFX format reference. The default EPUB engine uses Zachary Denton's boko. [KFX Input](https://www.mobileread.com/forums/showthread.php?t=291290) · [boko](https://github.com/zacharydenton/boko)

This project is licensed under GPL-3.0-only. See [LICENSE](LICENSE) and [third-party notices](THIRD-PARTY-NOTICES.md).
