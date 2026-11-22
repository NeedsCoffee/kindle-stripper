# How Kindle-stripper works

Kindle-stripper is a Rust CLI for the packaged Kindle for Windows app. It reads local downloads, opens supported Kindle delivery wrappers, and either preserves the resulting KFX components or converts their contents. The [README](../README.md#attribution-and-license) credits the upstream reverse engineering and format references. The Rust implementation combines that work with Windows-native credential access and local export.

## Follow one book through the pipeline

| Stage | Input | Output | Implementation |
| --- | --- | --- | --- |
| Select | ASIN, ASIN list, folder, or `--all` | One or more book directories | [main.rs](../src/main.rs) |
| Collect | Directory entries | Main KFX component and companions | [main.rs](../src/main.rs) |
| Open protected delivery | Local Kindle state plus vouchers | Content keys indexed by voucher ID | [kindle_store.rs](../src/kindle_store.rs), [voucher.rs](../src/voucher.rs), [voucher_mixer.rs](../src/voucher_mixer.rs) |
| Read components | DRMION or unwrapped KFX files | Individual `CONT` containers | [drmion.rs](../src/drmion.rs), [ion.rs](../src/ion.rs) |
| Package or convert | KFX containers | KFX-ZIP, EPUB 3, or PDF | [main.rs](../src/main.rs), [boko_epub.rs](../src/boko_epub.rs), [epub.rs](../src/epub.rs), [kfx_pdf.rs](../src/kfx_pdf.rs) |
| Publish | Completed temporary file | Final export at the requested path | [main.rs](../src/main.rs), [windows_paths.rs](../src/windows_paths.rs) |

## Selection, naming, and paths

`run` parses OS-native arguments and validates combinations before dispatch. An existing directory takes precedence over interpreting the argument as an ASIN or an ASIN list. Automatic discovery checks for the expected download folder and main filename, then sorts the results. An explicit list uses the caller's order and resolves every ASIN before starting. Batches process books sequentially so the reported position and failure belong to a specific ASIN.

`collect_components` selects regular files with `.azw`, `.md`, and `.res` extensions, sorts them, and requires exactly one main file. Vouchers are read separately when any selected component starts with DRMION magic. A component's extension alone cannot tell whether it is wrapped.

[book_names.rs](../src/book_names.rs) tries metadata companions first. Listing can read an unwrapped title immediately and loads credentials lazily when it encounters wrapped metadata. It reuses the credential lookup across the listing and derives keys per book when needed. If metadata fails, it still prints the ASIN. Export names use the title, first author and ASIN where available, budget the generated name in UTF-16 units, and substitute safe characters for Windows filename restrictions.

`windows_paths` checks Unicode, drive prefixes, invalid characters, and DOS device names before I/O. The export directory is canonicalised and checked against the Kindle `LocalState` ancestor chain. Resolving junctions first lets the check and the final write use the same resolved destination.

## Windows storage and voucher derivation

The protected path starts at `%LOCALAPPDATA%\Packages\AMZNKindle.AmazonKindleReadingApp_m1sc522ngdk36\LocalState`. It stays inside the current Windows account because the Kindle registration and storage key are tied to local Windows state.

1. **Unwrap the storage master.** `kindle_store` reads `Classic/Data/storage/main_shared.blob` and `main_shared.salt`. Windows CNG opens the `KindleForPC_SS` key, trying the platform provider first and the software provider second. RSA-OAEP with SHA-256 opens the blob while the private key stays in its Windows provider. The first 32 plaintext bytes and Kindle's salt feed HKDF-SHA256 to derive the storage key.
2. **Read the needed records.** The `.kinf2024` parser reverses the storage text transforms, including character shifts, Base64, record permutations and LZW encoding. It identifies records by Kindle's encoded hashes of field names and selects the DSN, account tokens and metrics checksum. A second HKDF derives a separate AES-256-GCM key for each selected record. The authentication tag is checked before using its plaintext.
3. **Resolve the client ID.** If the secure store lacks a DSN record, Windows DPAPI opens the Base64-wrapped `registration_data` JSON in the current user's context. The reader checks the resulting DSN before passing it to the voucher logic.
4. **Open each voucher.** `voucher` parses the Ion structure and supports desktop selector `0x8A`. It filters candidates for the account and client locks declared by the voucher. `voucher_mixer` combines the candidate inputs and voucher ID into the vector used by HMAC-SHA256 over `PIDv3`. The derived material is XORed with the issuance mask and used for the AES-256-CBC voucher keyset. The parser looks for the expected content key and records it as a 16-byte key under the voucher ID. Different keys for a repeated ID cause an error.

Those transformations implement the supported delivery format. This is why the installed app and the matching Windows user matter even when a book folder is supplied explicitly. Account values and derived keys are used in memory during the command. The CLI has no keyfile input or export step.

The mixer seeds a 256-byte buffer from the ordered lock inputs, fills the remainder with a wrapping integer recurrence, and reads the result as 32 little-endian 64-bit lanes. Forward quarter, pair, and reverse quarter passes mix those lanes before a final cross-half fold. The fixed constants, signed byte loads, rotations and wrapping arithmetic reproduce the upstream reference. Synthetic reference vectors check that the port produces the same bytes.

## From DRMION pages to KFX containers

[ion.rs](../src/ion.rs) is a bounded, slice-based reader for the binary Ion values this implementation needs. It separates type, annotation and body, checks variable-length numbers and ranges, and exposes struct fields by numeric symbol ID.

`drmion` checks the wrapper framing, Ion version, ProtectedData symbol-table import and document type. It walks envelopes in stored order and skips delivery indexes, which are unnecessary for sequential reconstruction. Envelope metadata selects a key by key ID or voucher ID. If both resolve they must agree. A previous key carries forward only where metadata declares neither reference.

Encrypted pages use AES-128-CBC and PKCS#7 padding. Plain pages contribute their payload directly. Supported compressed pages are LZMA-decoded into the same bounded output. The parser requires an explicit end marker and the reconstructed bytes must begin with `CONT`. A file truncated exactly between pages could otherwise look like a complete sequence to a loop that stops at EOF.

The archive path retains each component's original filename inside the ZIP. It streams through unwrapped `CONT` files and buffers wrapped files for reconstruction. This preserves the component boundaries and avoids depending on either EPUB engine or the PDF writer.

## Read the book for conversion

Despite its filename, [kfx_pdf.rs](../src/kfx_pdf.rs) also supplies the shared KFX book model. The reader checks the `CONT` header, reads container information and the 24-byte entity index entries, and bounds each entity's offset and size. It indexes fragments by type and ID and records document symbol names for resolving media. Structured entities expose an Ion body after the `ENTY` header. Media fragments expose raw bytes.

The conversion route uses the fixed-layout flag when the book has a metadata fragment and uses embedded PDF presence as the fallback when that fragment is absent. Supported fixed-layout books enter the PDF path. Reflowable books enter the selected EPUB path. A fixed-layout book without the required PDF-backed data fails here because the reflowable writers cannot preserve its page design.

## PDF-backed print replicas

The PDF writer follows the KFX document's reading order through sections, templates and stories to page resources. Each resource supplies a media location and, when present, a zero-based page index. If a single PDF is already in the required order and its parsed page count matches, the writer copies its bytes. There is also a direct-copy path for a lone embedded PDF when the KFX document record is absent. Its reported count of one corresponds to the copied resource.

When assembly is required, [pdf_pages.rs](../src/pdf_pages.rs) imports PDF page objects and their dependencies with lopdf. It can also wrap supported JPEG page resources in PDF pages while retaining the JPEG data. `write_pdf` creates a page tree in KFX order and prunes unreferenced objects. This handles page reordering and mixed resources without rendering every page to a bitmap.

Import renumbers source objects to avoid collisions and copies inherited page attributes such as resources, page boxes and rotation before replacing the original page tree. Assembled pages omit annotations and page actions because they can reference the original tree. The byte-copy paths preserve the embedded PDF as delivered.

## Default EPUB engine

Kindle downloads can split KFX entities across several containers. Boko accepts a single container, so `MultipartBook` builds a synthetic `CONT` in memory. It checks that component symbol tables agree, collects entity bytes into one payload and emits a new index using the new offsets. It deduplicates byte-identical entities, rejects conflicting copies, and preserves repeated unnamed position-map fragments.

Where the document's local symbols collide with Boko's built-in shared symbol range, the adapter moves the locals above that range. It rewrites affected entity IDs and Ion symbol references together, including field IDs and annotations. Raw media and font entities bypass Ion rebasing. Boko then opens the synthetic input, supplies the spine and writes the EPUB.

The export path applies two post-processing steps before publishing:

- [epub_image_check.rs](../src/epub_image_check.rs) reads Boko's OPF manifest and checks its image entries. If a packaged entry is missing or fails the image check, repair follows the exact KFX media name or resource mapping. It restores the original bytes only when one source entity matches and its detected media type agrees. The final check walks local image references in XHTML and SVG and validates the corresponding packaged entries. It cannot repair a missing manifest mapping by guessing a resource.
- [boko_metadata.rs](../src/boko_metadata.rs) updates the package with available source metadata, including the ASIN identifier, publication date and cover marker when resolved, and adds a modified timestamp. It keeps the legacy NCX identifier aligned with the package identifier.

The final image check covers package membership, local references and supported image signatures or SVG markup. Full image decoding, visual comparison, and whole-book EPUB conformance are outside those checks. A book can pass them and still have rendering, layout or navigation issues.

## Built-in EPUB engine

`epub::write` builds EPUB 3 from the shared KFX book model. It follows the section and story records, resolves inline text or text-block references, converts supported content into XHTML and packages referenced media. It writes the EPUB container descriptor, package manifest and spine, and navigation document. It can use the source cover and book metadata when available.

[epub_style.rs](../src/epub_style.rs) maps recognised KFX properties to CSS. It validates measurements and splits paragraphs into styled and linked text ranges while escaping the text for XHTML. [epub_navigation.rs](../src/epub_navigation.rs) collects requested offsets, associates emitted anchors with rendered sections, resolves internal links after rendering, and builds navigation from the available contents records. This separate engine is useful when Boko cannot assemble a reflowable book or the two engines differ in how they render it. Its coverage is limited to the KFX features it implements.

## Publishing and limits

Both archive and conversion writers build a temporary file in the resolved destination directory. Only after the writer finishes and any applicable checks pass does `persist_noclobber` publish the file. The final no-clobber operation protects a file that appears after the initial existence check. Input files are opened for reading. A batch retains files already published even if a subsequent book fails.

Component reads and reconstructed DRMION output are each capped at 512 MiB. Voucher reads are capped at 64 KiB. The Boko adapter limits merged entity payloads to 256 MiB and caps the index at 100,000 entries. The code also bounds selected parser nesting, page counts and XML reads. These limits constrain malformed or unusually large inputs. Conversion can still use substantially more memory than the final file size because parsers, adapters and writers hold overlapping representations.

## Verify changes

The Windows MSVC configuration in `.cargo/config.toml` links the C runtime statically. The Rust dependencies are compiled into the executable, while Windows supplies CNG and DPAPI through system APIs.

Run `cargo test --locked --release` on Windows. Unit tests cover areas including Windows path handling, naming, voucher processing, image checks, EPUB styling and navigation. [parser_fuzz.rs](../src/parser_fuzz.rs) exercises mutated synthetic Ion and DRMION inputs, including compressed pages and truncation.

For a change to the export pipeline, run the CLI on representative local downloads and check each affected output. Compare archive entry names and component bytes, inspect EPUB text, navigation and images, and compare PDF pages and order with the source. Use a separate conformance checker when assessing EPUB conformance. Local integration checks depend on the Kindle version, Windows profile and sample formats used.
