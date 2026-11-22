# Set up Kindle-stripper on Windows

Kindle-stripper reads downloads from the packaged Kindle for Windows app. The tested app version is **1.1.5634.0**. Compatibility with other versions and Kindle installations that use different storage layouts is unverified.

Use the tool with books you already have access to through your own Kindle account. Keep exports for personal use.

This guide describes the version in the source tree. If the downloadable release predates 0.200.0, [build from source](#build-from-source) for the interactive prompts and embedded icon.

## Run the Windows x64 release

1. Install Kindle for Windows, sign in, and download a book. A title visible in your cloud library still needs a local download.
2. Download the Windows x64 ZIP and extract it to a folder you can write to, such as a folder under Documents. [Kindle-stripper releases](https://github.com/NeedsCoffee/kindle-stripper/releases)
3. Open PowerShell under the same Windows user that downloaded the book. Change to the extracted folder and run:

   ```powershell
   .\kindle-stripper.exe --help
   .\kindle-stripper.exe
   ```

Help displays the version and syntax before Kindle is configured. The second command reads the local download list. In builds from version 0.200.0, double-click the executable or run `.\kindle-stripper.exe -i` for guided export. PowerShell uses `.\` to run an executable from the current directory. If you keep the executable elsewhere, use the call operator with its quoted path:

```powershell
& 'C:\Tools\Kindle-stripper\kindle-stripper.exe' --help
```

The executable uses the current user's local Kindle registration and Windows cryptography. It needs no Calibre, Python, separate converter, or exported key file. Copying a protected book folder from another installation alone may leave the required Windows and Kindle account state unavailable.

## Check that Kindle has downloaded the book

The app's automatic discovery looks under the following directory. To check whether the directory exists without opening any registration or credential files:

```powershell
$content = Join-Path $env:LOCALAPPDATA 'Packages\AMZNKindle.AmazonKindleReadingApp_m1sc522ngdk36\LocalState\Classic\Content'
Test-Path -LiteralPath $content -PathType Container
```

If the result is `False`, confirm that you are in the Windows profile that runs the packaged Kindle app and that Kindle has completed a download. If it is `True` but Kindle-stripper lists no titles, see the discovery rules in the [usage guide](usage.md).

## Build from source

Install Rust with rustup and use the Windows MSVC toolchain. Install the Visual Studio C++ build tools with the MSVC compiler and Windows SDK. Open a fresh PowerShell after installation. The project uses Rust edition 2024 and the release build was verified with Rust 1.100.0. [Rust installation guide](https://rust-lang.org/tools/install/)

From the repository root:

```powershell
rustc --version
rustup show active-toolchain
cargo test --locked --release
cargo build --locked --release
.\target\release\kindle-stripper.exe --help
```

The executable is at `target\release\kindle-stripper.exe`. Cargo uses the checked-in dependency versions through `--locked`. The build uses the Windows SDK resource compiler to embed the checked-in icon. If the build cannot find `link.exe` or `rc.exe`, check that the MSVC build tools and Windows SDK are installed, then retry from a Visual Studio developer shell. If PowerShell cannot resolve `cargo` or `rustup`, reopen the shell and check the Rust installation.

Continue with [usage examples](usage.md) once help and listing work.
