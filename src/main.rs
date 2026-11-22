mod boko_epub;
mod boko_metadata;
mod book_names;
mod drmion;
mod epub;
mod epub_image_check;
mod epub_navigation;
mod epub_style;
mod interactive;
mod ion;
mod kfx_pdf;
mod kindle_store;
#[cfg(test)]
mod parser_fuzz;
mod pdf_pages;
mod voucher;
mod voucher_mixer;
mod windows_paths;

use anyhow::{Context, Result, bail, ensure};
use drmion::ContentKeys;
use std::{
    env,
    ffi::OsString,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::ExitCode,
};
use tempfile::NamedTempFile;
use zip::{ZipWriter, write::SimpleFileOptions};

const USAGE: &str = "kindle-stripper                           List downloaded ASINs and titles
kindle-stripper --interactive, -i          Choose export options interactively
kindle-stripper <ASIN | book-folder> [--convert [--epub-engine <boko|basic>]] [--output <path> | --output-folder <directory>] [--quiet]
kindle-stripper <ASIN,ASIN,... | --all> [--output-folder <directory>] [--convert [--epub-engine <boko|basic>]] [--quiet]
  --all        Export all locally downloaded books.
  --output-folder <directory>  Write to an existing directory. Default: current directory.
  --convert    Export print replicas as PDF and reflowable books as EPUB 3.
  --epub-engine <boko|basic>  Choose an EPUB exporter. Default: boko.
  -q, --quiet  Suppress progress and success messages. With no book selected, do nothing.";
const HELP_HEADER: &str = concat!(
    env!("CARGO_PKG_NAME"),
    " ",
    env!("CARGO_PKG_VERSION"),
    "\n",
    env!("CARGO_PKG_DESCRIPTION"),
    "\nhttps://github.com/Satsuoni/DeDRM_tools/",
);
const MAX_COMPONENT: u64 = 512 * 1024 * 1024;

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let pause_on_exit = args.is_empty() && interactive::has_dedicated_console();
    let explicit_interactive = matches!(
        args.as_slice(),
        [flag] if matches!(flag.to_str(), Some("--interactive" | "-i"))
    );
    let result = if pause_on_exit || explicit_interactive {
        interactive::run()
    } else {
        run(args, false)
    };
    let exit_code = match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error:#}");
            ExitCode::FAILURE
        }
    };
    if pause_on_exit {
        interactive::pause_on_exit();
    }
    exit_code
}

fn run(args: impl IntoIterator<Item = OsString>, create_output_folder: bool) -> Result<()> {
    let mut args = args.into_iter();
    let mut source = None;
    let mut output = None;
    let mut output_folder = None;
    let mut all = false;
    let mut quiet = false;
    let mut convert = false;
    let mut epub_engine = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help" | "-h") => {
                println!("{HELP_HEADER}\n\n{USAGE}");
                return Ok(());
            }
            Some("--quiet" | "-q") => quiet = true,
            Some("--interactive" | "-i") => bail!("use --interactive or -i on its own"),
            Some("--verbose" | "-v") => {}
            Some("--convert") => convert = true,
            Some("--all") => all = true,
            Some("--output-folder") => {
                ensure!(output_folder.is_none(), "duplicate --output-folder option");
                let path = PathBuf::from(args.next().context("missing output directory")?);
                windows_paths::validate(&path, false).context("invalid output directory")?;
                output_folder = Some(path);
            }
            Some("--epub-engine") => {
                ensure!(epub_engine.is_none(), "duplicate --epub-engine option");
                epub_engine = Some(
                    match args.next().as_deref().and_then(|value| value.to_str()) {
                        Some("basic" | "native") => false,
                        Some("boko") => true,
                        _ => bail!("--epub-engine requires boko or basic"),
                    },
                );
            }
            Some("--output") => {
                ensure!(output.is_none(), "duplicate --output option");
                let path = PathBuf::from(args.next().context("missing output path")?);
                windows_paths::validate(&path, true).context("invalid output path")?;
                output = Some(path);
            }
            Some(option) if option.starts_with('-') => bail!("unknown option; usage: {USAGE}"),
            _ => {
                ensure!(source.is_none(), "provide one ASIN or book folder");
                source = Some(PathBuf::from(arg));
            }
        }
    }
    ensure!(
        epub_engine.is_none() || convert,
        "--epub-engine requires --convert"
    );
    ensure!(
        output.is_none() || output_folder.is_none(),
        "--output and --output-folder cannot be combined"
    );
    let options = ExportOptions {
        convert,
        use_boko: epub_engine.unwrap_or(true),
        verbose: !quiet,
    };
    if create_output_folder && let Some(directory) = &output_folder {
        create_export_directory(directory)?;
    }
    if all {
        ensure!(
            source.is_none() && output.is_none(),
            "--all cannot be combined with a book selector or --output"
        );
        let directory = output_folder.as_deref().unwrap_or_else(|| Path::new("."));
        return export_all(downloaded_books()?, directory, options);
    }
    let Some(mut folder) = source else {
        ensure!(
            output.is_none() && output_folder.is_none(),
            "output options require an ASIN, book folder, or --all"
        );
        if quiet {
            return Ok(());
        }
        ensure!(!convert, "--convert requires an ASIN or book folder");
        list_downloads(&downloaded_books()?, false);

        return Ok(());
    };
    windows_paths::validate(&folder, false).context("invalid book selector or folder path")?;

    if !folder.is_dir()
        && let Some(selector) = folder.as_os_str().to_str()
        && selector.contains(',')
        && !selector.contains(['\\', '/'])
    {
        ensure!(output.is_none(), "use --output-folder for a list of ASINs");
        let asins = parse_asin_list(selector)?;
        let mut downloads: std::collections::HashMap<_, _> =
            downloaded_books()?.into_iter().collect();
        let books = asins
            .into_iter()
            .map(|asin| {
                let folder = downloads.remove(&asin).with_context(|| {
                    format!("ASIN {asin} is not downloaded in the Windows Kindle app")
                })?;
                Ok((asin, folder))
            })
            .collect::<Result<Vec<_>>>()?;
        let directory = output_folder.as_deref().unwrap_or_else(|| Path::new("."));
        return export_all(books, directory, options);
    }

    if !folder.is_dir()
        && let Some(asin) = folder.as_os_str().to_str().filter(|value| is_asin(value))
    {
        folder = downloaded_books()?
            .into_iter()
            .find(|(downloaded, _)| downloaded.eq_ignore_ascii_case(asin))
            .map(|(_, path)| path)
            .with_context(|| format!("ASIN {asin} is not downloaded in the Windows Kindle app"))?;
    }
    export_book(&folder, None, output, output_folder.as_deref(), options)
}

#[derive(Clone, Copy)]
struct ExportOptions {
    convert: bool,
    use_boko: bool,
    verbose: bool,
}

fn export_all(
    books: Vec<(String, PathBuf)>,
    directory: &Path,
    options: ExportOptions,
) -> Result<()> {
    resolve_export_directory(directory)?;
    let count = books.len();
    let mut failed = 0;
    if options.verbose {
        eprintln!(
            "Exporting {count} downloaded Kindle books to {}.",
            windows_paths::display(directory)
        );
    }
    for (index, (asin, folder)) in books.into_iter().enumerate() {
        if options.verbose {
            eprintln!("\nBook {}/{count} ({asin})", index + 1);
        }
        if let Err(error) = export_book(&folder, Some(&asin), None, Some(directory), options) {
            eprintln!("Error exporting {asin}: {error:#}");
            failed += 1;
        }
    }
    ensure!(failed == 0, "{failed} of {count} books failed to export");
    if options.verbose {
        eprintln!("\nExported {count} books.");
    }
    Ok(())
}

fn export_book(
    folder: &Path,
    asin: Option<&str>,
    output: Option<PathBuf>,
    directory: Option<&Path>,
    options: ExportOptions,
) -> Result<()> {
    let ExportOptions {
        convert,
        use_boko,
        verbose,
    } = options;
    let folder = windows_paths::canonicalize_local(folder).context("invalid book folder")?;
    let (main, components) = collect_components(&folder)?;
    if verbose {
        eprintln!("Book folder: {}", windows_paths::display(&folder));
        eprintln!("Found {} KFX components.", components.len());
    }
    let filename = main.file_stem().context("invalid main component name")?;
    let keys = if has_encrypted_component(&components)? {
        if verbose {
            eprintln!("Reading local Kindle credentials through Windows.");
        }
        voucher::derive_keys(&folder, &kindle_store::credential_database()?, verbose)?
    } else {
        if verbose {
            eprintln!("No DRMION-wrapped components found.");
        }
        ContentKeys::new()
    };
    let default_output = |extension: &str, metadata: Option<book_names::BookName>| {
        let name = book_names::filename(metadata, asin, filename, extension);
        directory.map_or_else(|| name.clone(), |directory| directory.join(&name))
    };
    let output = if convert {
        let book = read_conversion_book(&components, &keys)?;
        let print_replica = epub::is_fixed_layout(&book)?;
        ensure!(
            !print_replica || book.has_pdf(),
            "fixed-layout KFX has no supported PDF-backed pages"
        );
        let extension = if print_replica { "pdf" } else { "epub" };
        let output =
            output.unwrap_or_else(|| default_output(extension, book_names::from_book(&book)));
        let destination = resolve_export_path(&output)?;
        ensure!(
            !destination.exists(),
            "output already exists: {}",
            windows_paths::display(&output)
        );
        let conversion = if print_replica {
            ConversionBook::Pdf(book)
        } else if use_boko {
            ConversionBook::Boko(
                book,
                read_components(
                    &components,
                    &keys,
                    boko_epub::MultipartBook::default(),
                    boko_epub::MultipartBook::add,
                )?,
            )
        } else {
            ConversionBook::BasicEpub(book)
        };
        export_converted(conversion, &destination, filename, verbose)?;
        output
    } else {
        let output = output
            .unwrap_or_else(|| default_output("kfx-zip", book_names::read(&components, &keys)));
        export(&components, &keys, &resolve_export_path(&output)?, verbose)?;
        output
    };
    if verbose {
        println!("Wrote {}", windows_paths::display(&output));
    }
    Ok(())
}

fn parse_asin_list(selector: &str) -> Result<Vec<String>> {
    let mut seen = std::collections::HashSet::new();
    selector
        .split(',')
        .map(|value| {
            let value = value.trim();
            ensure!(
                is_asin(value),
                "each comma-separated ASIN must contain 10 letters or digits"
            );
            let asin = value.to_ascii_uppercase();
            ensure!(seen.insert(asin.clone()), "duplicate ASIN {asin}");
            Ok(asin)
        })
        .collect()
}

fn is_asin(value: &str) -> bool {
    value.len() == 10 && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn list_downloads(books: &[(String, PathBuf)], numbered: bool) {
    eprintln!("Found {} downloaded Kindle books.", books.len());
    if books.is_empty() {
        println!("No downloaded Kindle books found.");
    }
    book_names::list(books, numbered);
}

fn downloaded_books() -> Result<Vec<(String, PathBuf)>> {
    let content = kindle_store::local_state()?.join("Classic/Content");
    let entries = fs::read_dir(&content).with_context(|| {
        format!(
            "opening Windows Kindle downloads at {}",
            windows_paths::display(&content)
        )
    })?;
    let mut books = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some((asin, suffix)) = name.split_once('_') else {
            continue;
        };
        if !is_asin(asin) || !suffix.eq_ignore_ascii_case("EBOK") {
            continue;
        }
        let path = entry.path();
        if path.join(format!("{name}.azw")).is_file() {
            books.push((asin.to_ascii_uppercase(), path));
        }
    }
    books.sort_unstable();
    Ok(books)
}

fn has_encrypted_component(components: &[PathBuf]) -> Result<bool> {
    for component in components {
        let mut header = Vec::with_capacity(drmion::MAGIC.len());
        fs::File::open(component)?
            .take(drmion::MAGIC.len() as u64)
            .read_to_end(&mut header)?;
        if header == drmion::MAGIC {
            return Ok(true);
        }
    }
    Ok(false)
}

fn collect_components(folder: &Path) -> Result<(PathBuf, Vec<PathBuf>)> {
    let mut main = None;
    let mut components = Vec::new();
    for entry in fs::read_dir(folder)
        .with_context(|| format!("reading {}", windows_paths::display(folder)))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match extension.as_str() {
            "azw" => {
                ensure!(main.is_none(), "multiple .azw files in book folder");
                main = Some(path.clone());
            }
            "md" | "res" => {}
            _ => continue,
        }
        components.push(path);
    }
    components.sort();
    Ok((main.context("no .azw file in book folder")?, components))
}

fn export(components: &[PathBuf], keys: &ContentKeys, output: &Path, verbose: bool) -> Result<()> {
    use std::io::Write;

    ensure!(
        !output.exists(),
        "output already exists: {}",
        windows_paths::display(output)
    );
    let directory = output.parent().context("missing output directory")?;
    let mut temporary = NamedTempFile::new_in(directory)?;
    let mut archive = ZipWriter::new(temporary.as_file_mut());
    for path in components {
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("non-Unicode KFX component name")?;
        windows_paths::validate_component(filename).context("invalid KFX component name")?;
        let mut component = fs::File::open(path)?.take(MAX_COMPONENT + 1);
        let mut header = Vec::with_capacity(drmion::MAGIC.len());
        (&mut component)
            .take(drmion::MAGIC.len() as u64)
            .read_to_end(&mut header)?;
        archive.start_file(filename, SimpleFileOptions::default())?;
        if header == drmion::MAGIC {
            let mut bytes = header;
            component.read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 <= MAX_COMPONENT,
                "{filename} exceeds size limit"
            );
            let plaintext = drmion::decrypt(&bytes, keys).with_context(|| filename.to_owned())?;
            archive.write_all(&plaintext)?;
            if verbose {
                eprintln!(
                    "Decoded {filename:?} ({} bytes into {} bytes).",
                    bytes.len(),
                    plaintext.len()
                );
            }
        } else {
            ensure!(
                header.starts_with(b"CONT"),
                "{filename} is not a supported KFX container"
            );
            archive.write_all(&header)?;
            let copied = io::copy(&mut component, &mut archive)?;
            let size = copied + header.len() as u64;
            ensure!(size <= MAX_COMPONENT, "{filename} exceeds size limit");
            if verbose {
                eprintln!("Copied unwrapped {filename:?} ({size} bytes).");
            }
        }
    }
    archive.finish()?;
    if verbose {
        eprintln!("Archive finalized. Publishing without overwriting an existing file.");
    }
    temporary
        .persist_noclobber(output)
        .with_context(|| format!("saving {}", windows_paths::display(output)))?;
    Ok(())
}

fn read_conversion_book(components: &[PathBuf], keys: &ContentKeys) -> Result<kfx_pdf::Book> {
    read_components(
        components,
        keys,
        kfx_pdf::Book::default(),
        kfx_pdf::Book::add_component,
    )
}

fn read_components<T>(
    components: &[PathBuf],
    keys: &ContentKeys,
    mut book: T,
    mut add: impl FnMut(&mut T, &[u8]) -> Result<()>,
) -> Result<T> {
    for path in components {
        let bytes = read_component(path, keys)?;
        add(&mut book, &bytes)
            .with_context(|| format!("reading {}", windows_paths::display(path)))?;
    }
    Ok(book)
}

fn read_component(path: &Path, keys: &ContentKeys) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_COMPONENT + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_COMPONENT,
        "KFX component exceeds size limit"
    );
    if bytes.starts_with(drmion::MAGIC) {
        bytes = drmion::decrypt(&bytes, keys)
            .with_context(|| format!("decoding {}", windows_paths::display(path)))?;
    }
    Ok(bytes)
}

enum ConversionBook {
    Pdf(kfx_pdf::Book),
    BasicEpub(kfx_pdf::Book),
    Boko(kfx_pdf::Book, boko_epub::MultipartBook),
}

fn export_converted(
    book: ConversionBook,
    output: &Path,
    filename: &std::ffi::OsStr,
    verbose: bool,
) -> Result<()> {
    let mut temporary =
        NamedTempFile::new_in(output.parent().context("missing output directory")?)?;
    match book {
        ConversionBook::Pdf(book) => {
            let count = book.write_pdf(temporary.as_file_mut())?;
            if verbose {
                eprintln!("Exported {count} PDF pages.");
            }
        }
        ConversionBook::Boko(source, book) => {
            book.write(temporary.as_file_mut())?;
            let repaired = epub_image_check::repair_boko_images(temporary.as_file_mut(), &source)
                .context("repairing boko EPUB images")?;
            boko_metadata::apply(
                temporary.as_file_mut(),
                &source,
                &filename.to_string_lossy(),
            )
            .context("updating boko EPUB metadata")?;
            epub_image_check::check_boko_images(temporary.as_file_mut())
                .context("boko EPUB image validation failed")?;
            if verbose {
                if repaired != 0 {
                    eprintln!("Restored {repaired} EPUB image resources from KFX.");
                }
                eprintln!("Built EPUB 3 publication with boko.");
            }
        }
        ConversionBook::BasicEpub(book) => {
            epub::write(&book, temporary.as_file_mut(), &filename.to_string_lossy())?;
            if verbose {
                eprintln!("Built EPUB 3 publication.");
            }
        }
    }
    temporary
        .persist_noclobber(output)
        .with_context(|| format!("saving {}", windows_paths::display(output)))?;
    Ok(())
}

fn resolve_export_path(output: &Path) -> Result<PathBuf> {
    windows_paths::validate(output, true).context("invalid output path")?;
    let filename = output.file_name().context("missing output filename")?;
    let directory = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    Ok(resolve_export_directory(directory)?.join(filename))
}

fn resolve_export_directory(directory: &Path) -> Result<PathBuf> {
    windows_paths::validate(directory, false).context("invalid output directory")?;
    resolve_existing_export_directory(directory)
}

fn resolve_existing_export_directory(directory: &Path) -> Result<PathBuf> {
    // Resolve junctions before checking the destination and use that same
    // directory for both the temporary archive and its final name.
    let directory =
        windows_paths::canonicalize_local(directory).context("invalid output directory")?;
    ensure!(directory.is_dir(), "output directory must be a directory");
    match kindle_store::local_state()?.canonicalize() {
        Ok(state) => ensure!(
            !directory
                .ancestors()
                .any(|ancestor| ancestor.as_os_str().eq_ignore_ascii_case(state.as_os_str())),
            "output directory is inside Windows Kindle app data"
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("resolving Windows Kindle app data"),
    }
    Ok(directory)
}

fn create_export_directory(directory: &Path) -> Result<()> {
    windows_paths::validate(directory, false).context("invalid output directory")?;
    let mut ancestor = directory;
    let mut missing = Vec::new();
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(
                    ancestor
                        .file_name()
                        .context("invalid new output directory")?,
                );
                ancestor = ancestor
                    .parent()
                    .filter(|path| !path.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
            }
            Err(error) => return Err(error).context("checking output directory"),
        }
    }
    let mut resolved = resolve_export_directory(ancestor)?;
    for part in missing.into_iter().rev() {
        resolved.push(part);
        match fs::create_dir(&resolved) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "creating output directory {}",
                        windows_paths::display(&resolved)
                    )
                });
            }
        }
        // Recheck an existing directory if another process created it first.
        resolved = resolve_existing_export_directory(&resolved)?;
    }
    Ok(())
}

#[cfg(test)]
mod export_directory_tests {
    use super::*;

    #[test]
    fn interactive_output_creates_nested_folders_and_rejects_file_parents() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let destination = temporary.path().join("Kindle Exports/Nested");
        assert!(resolve_export_directory(&destination).is_err());
        create_export_directory(&destination)?;
        create_export_directory(&destination)?;
        assert!(destination.is_dir());

        let file = destination.join("existing");
        fs::write(&file, "keep")?;
        assert!(create_export_directory(&file.join("nested")).is_err());
        assert_eq!(fs::read_to_string(file)?, "keep");
        Ok(())
    }
}
