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

## Read local Kindle credentials

```mermaid
flowchart TD
    State["Locate the current Windows user's Kindle LocalState"] --> Blob["Read main_shared.blob and main_shared.salt"]
    Blob --> CNG["Ask Windows CNG to open the storage master with KindleForPC_SS"]
    CNG --> StorageKey["Derive the storage key from the master and salt"]
    State --> Records["Decode the .kinf2024 record structure"]
    Records --> Select["Select DSN, account-token, and metrics-checksum records by field identifier"]
    Select --> Open["Derive a key for each selected record and open it with AES-GCM"]
    StorageKey --> Open
    Open --> DSN{"DSN present?"}
    DSN -->|"Yes"| Result["Return client ID and available account records in memory"]
    DSN -->|"No"| DPAPI["Open registration_data through Windows DPAPI and read the DSN"]
    DPAPI --> Result
```

CNG tries the platform provider before the software provider. The private key stays with Windows. AES-GCM checks each selected record's authentication tag. The stage also requires at least one supported account record. See [kindle_store.rs](../src/kindle_store.rs).

## Derive content keys from vouchers

```mermaid
flowchart TD
    Files["Read each .voucher beside the book"] --> Parse["Parse the Ion voucher and check its desktop selector"]
    Parse --> Locks["Prepare candidates for the declared account and client locks"]
    Credentials["Local client ID and account records"] --> Locks
    Parse --> Mask["Open the issuance record to obtain its mask"]
    Locks --> Candidate{"Another candidate combination?"}
    Candidate -->|"Yes"| Mix["Mix the candidate inputs and voucher ID"]
    Mix --> HMAC["Run HMAC-SHA256 over PIDv3"]
    HMAC --> Combine["XOR the HMAC output with the issuance mask"]
    Mask --> Combine
    Combine --> Decrypt["Open the voucher ciphertext with AES-256-CBC"]
    Decrypt --> Keyset{"Usable Ion keyset?"}
    Keyset -->|"No"| Candidate
    Keyset -->|"Yes"| Save["Index the 16-byte content key by voucher ID"]
    Candidate -->|"No"| Error["Report that no credential combination opened the voucher"]
    Save --> Pages["DRMION metadata uses the key map to select the page key"]
```

A lock that the voucher does not declare contributes no value. The Ion keyset can carry the content key directly or in an inner wrapped record. Conflicting keys for the same voucher ID stop the export. See [voucher.rs](../src/voucher.rs), [voucher_mixer.rs](../src/voucher_mixer.rs), and [drmion.rs](../src/drmion.rs).
