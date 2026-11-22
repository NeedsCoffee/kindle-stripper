# Use Kindle-stripper

These examples use PowerShell from the directory containing `kindle-stripper.exe`. Substitute ASINs from your own Kindle listing. See [setup](setup.md) for installation and account requirements.

## List downloads and create an export folder

```powershell
.\kindle-stripper.exe --help
.\kindle-stripper.exe

$exports = Join-Path $env:USERPROFILE 'Documents\Kindle Exports'
New-Item -ItemType Directory -Path $exports -Force | Out-Null
```

Listing scans the Kindle download directory for `<ASIN>_EBOK` folders with a matching `<ASIN>_EBOK.azw` main file. It displays an ASIN even if it cannot read the title. Discovery only confirms that the main file is present. Missing companions, unsupported data, or unavailable credentials can still cause export failures.

## Export one book

```powershell
# Save the component archive in the current directory.
.\kindle-stripper.exe B008E8XV56

# Save the component archive in the export folder.
.\kindle-stripper.exe B008E8XV56 --output-folder "$exports"

# Convert and use a filename generated from book metadata.
.\kindle-stripper.exe B008E8XV56 --convert --output-folder "$exports"

# Give a component archive an exact filename.
.\kindle-stripper.exe B008E8XV56 --output "$exports\My backup.kfx-zip"
```

| Selection | Result | When to use it |
| --- | --- | --- |
| Default | `.kfx-zip` containing KFX components | Keep the original KFX structure in an archive or use another KFX-aware tool. |
| `--convert` on a supported reflowable book | `.epub` | Read reflowable text in an EPUB reader. |
| `--convert` on a supported PDF-backed print replica | `.pdf` | Preserve fixed pages. |

Conversion chooses the format from the book. A fixed-layout KFX without supported PDF-backed pages produces an error. The extension you supply with `--output` only names the file. When you are unsure which conversion format applies, use `--output-folder` and let the tool choose the extension.

You can also pass a complete book folder directly. The folder needs exactly one `.azw` and the original `.md`, `.res`, and `.voucher` companions where applicable:

```powershell
$content = Join-Path $env:LOCALAPPDATA 'Packages\AMZNKindle.AmazonKindleReadingApp_m1sc522ngdk36\LocalState\Classic\Content'
$bookFolder = Join-Path $content 'B008E8XV56_EBOK'
.\kindle-stripper.exe "$bookFolder" --output-folder "$exports"
```

## Choose an EPUB engine

Boko is the default. If it fails or the output has layout or navigation problems, try the built-in engine and compare the resulting EPUBs. Layout and navigation fidelity vary by book. Use the named output in the second example for a book you know is reflowable:

```powershell
.\kindle-stripper.exe B008E8XV56 --convert --epub-engine boko --output-folder "$exports"
.\kindle-stripper.exe B008E8XV56 --convert --epub-engine basic --output "$exports\Comparison - basic.epub"
```

`--epub-engine` requires `--convert` and only affects reflowable EPUB conversion. The legacy value `native` also selects `basic`. Engine failure ends that book's export. To try the other engine, run it explicitly.

## Export multiple books

```powershell
# Export two books in the order supplied.
.\kindle-stripper.exe 'B008E8XV56,B00XLII45G' --output-folder "$exports"

# Convert specific books, choosing EPUB or PDF for each.
.\kindle-stripper.exe 'B008E8XV56,B00XLII45G' --convert --output-folder "$exports"

# Convert all discovered downloads, in ASIN order.
.\kindle-stripper.exe --all --convert --output-folder "$exports"
```

Quote the comma-separated selector so PowerShell passes one argument. Each entry must contain ten ASCII letters or digits. Matching is case-insensitive, surrounding spaces are trimmed, and duplicates are rejected. All requested ASINs must resolve before the list begins exporting. Use `--output-folder` for picks or `--all`. `--output` applies to a single book.

Once a batch begins, the tool reports per-book errors, continues with later books, and returns a nonzero exit code if any book failed. Successful files stay in place. Existing files count as errors. To retry, select only the failed ASINs or choose a new destination or filename as appropriate.

## Names, paths, and scripts

The default name is similar to `Example Title - First Author [B008E8XV56].epub`. The tool sanitises Windows filenames, shortens long metadata values and falls back to an ASIN or source name when it lacks a usable title.

Use relative paths or drive-qualified local paths such as `C:\Exports`. Quote paths with spaces. Output directories must already exist and must be outside the Kindle app's `LocalState`, including after junction resolution. Outputs default to the current directory. Use `--output` or `--output-folder` on a single export.

```powershell
.\kindle-stripper.exe --all --convert --output-folder "$exports" --quiet
$exportExitCode = $LASTEXITCODE
if ($exportExitCode -ne 0) {
    Write-Warning "At least one export failed or the batch could not start (exit $exportExitCode)."
}
```

`--quiet` (or `-q`) suppresses progress and success messages and keeps errors visible. With no book selected, quiet mode suppresses listing. Use `--help` (or `-h`) for syntax.

## Troubleshoot an export

| Symptom | What to check |
| --- | --- |
| Download directory cannot be opened | Confirm the packaged Kindle app is set up under the current Windows user and has downloaded a book. |
| ASIN is missing or listing lacks a title | Check the book is local and in the expected folder layout. A missing title can mean metadata could not be read even though the download was discovered. |
| No `.azw`, multiple `.azw` files, or missing voucher | Pass the individual book folder and keep its original companions together. |
| Windows could not open the Kindle storage key or registration | Run under the Windows user that downloaded the book. Check that Kindle is still signed in on that installation. |
| Unsupported voucher selector or KFX container | The tool supports specific Kindle delivery formats. Check the [tested setup](setup.md) and that the source is a KFX download. |
| Output already exists | Pick another destination or filename. A successful previous export is preserved. |
| Boko conversion or image validation failed | Try `--epub-engine basic` for a reflowable book, or export KFX-ZIP to retain the source component structure. |
| Fixed-layout KFX has no supported PDF-backed pages | Use KFX-ZIP to export the components if the delivery is otherwise supported. |

Open a converted EPUB or PDF in your reader and check the reading order, images, and navigation you rely on before treating it as a usable copy. See [how it works](implementation.md) for the checks performed during conversion.
