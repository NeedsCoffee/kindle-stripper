# Kindle-stripper code flow

The diagrams follow the CLI entry point and the export of an individual book. See [How Kindle-stripper works](implementation.md) for the source map and design rationale.

## Command dispatch

```mermaid
flowchart TD
    Start["Parse command-line arguments"] --> Mode{"Command"}
    Mode -->|"--help or -h"| Help["Print command syntax"]
    Mode -->|"No selector"| Quiet{"Quiet mode?"}
    Quiet -->|"Yes"| Done["Exit successfully"]
    Quiet -->|"No"| List["Discover downloads and attempt to read titles"]
    Mode -->|"One ASIN or folder"| Select["Resolve the ASIN or use the supplied folder"]
    Mode -->|"ASIN list or --all"| Batch["Resolve the full selection and check the output folder"]
    Select --> Export["Export the selected book"]
    Batch --> Loop["Export each book in order"]
    Loop --> Status["Report per-book errors and return failure if any book failed"]
```

Discovery checks for the expected folder and main filename. Listing falls back to the ASIN when metadata is unavailable. A supplied ASIN list must resolve in full before the batch starts.

## Read a book and choose its output

```mermaid
flowchart TD
    Folder["Collect the .azw, .md and .res components"] --> Wrapped{"Any DRMION wrapper?"}
    Wrapped -->|"Yes"| Store["Read credentials from local Kindle and Windows storage"]
    Store --> Voucher["Derive content keys from the book's vouchers"]
    Voucher --> Read["Read KFX components, opening wrappers as needed"]
    Wrapped -->|"No"| Read
    Read --> Convert{"Convert requested?"}
    Convert -->|"No"| Archive["Package the components as KFX-ZIP"]
    Convert -->|"Yes"| Book["Index KFX entities and read layout metadata"]
    Book --> Layout{"Fixed layout?"}
    Layout -->|"Yes"| Embedded{"Embedded PDF present?"}
    Embedded -->|"Yes"| PDF["Copy or assemble PDF pages"]
    Embedded -->|"No"| Unsupported["Report unsupported fixed-layout book"]
    Layout -->|"No"| Engine{"EPUB engine"}
    Engine -->|"Boko, default"| Merge["Build a single KFX container for Boko"]
    Merge --> Boko["Write EPUB, attempt source image repair, update metadata, and validate images"]
    Engine -->|"Basic"| Basic["Write EPUB 3 from supported KFX content"]
    Archive --> Publish["Publish the finished temporary file without overwriting"]
    PDF --> Publish
    Boko --> Publish
    Basic --> Publish
```

The component-reading node represents work performed as needed by each output path. KFX-ZIP streams unwrapped components into the archive. Conversion builds a shared KFX index, and Boko also reads components into its own multipart adapter. Output writers use a temporary file in the destination directory. If any step fails, the book's export stops before publication. A batch continues with the next book.
