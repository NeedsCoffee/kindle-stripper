//! Reassemble a KFX container from the pages in its DRMION wrapper.

use crate::ion::{self, Value};
use aes::Aes128;
use anyhow::{Context, Result, bail, ensure};
use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use std::{
    collections::HashMap,
    io::{Cursor, Write},
};

pub const MAGIC: &[u8] = b"\xeaDRMION\xee";
const ION_VERSION: &[u8] = b"\xe0\x01\x00\xea";
const MAX_CONTAINER: usize = 512 * 1024 * 1024;

pub type ContentKeys = HashMap<String, [u8; 16]>;

// ProtectedData v1 symbol IDs from DeDRM's ion.py.
const DOCTYPE: usize = 50;
const ENDDOC: usize = 52;
const ENVELOPE: [usize; 2] = [10, 67];
const METADATA: [usize; 2] = [11, 68];
const ENCRYPTED_PAGE: [usize; 2] = [20, 69];
const PLAIN_PAGE: [usize; 3] = [40, 41, 70];
const COMPRESSED: usize = 72;
const INDEX_TABLE: usize = 25;
const INDEX_OFFSET: usize = 51;
const KEY_ID: usize = 14;
const VOUCHER_ID: usize = 16;
const CIPHER_TEXT: usize = 21;
const CIPHER_IV: usize = 22;
const DATA: usize = 24;

/// Decode envelopes sequentially and concatenate their plaintext pages in file
/// order. The output is the underlying KFX component beginning with CONT.
pub fn decrypt(input: &[u8], keys: &ContentKeys) -> Result<Vec<u8>> {
    // The delivery trailer occupies eight bytes and need not repeat the magic.
    let mut document = input
        .strip_prefix(MAGIC)
        .and_then(|body| body.get(..body.len().checked_sub(8)?))
        .and_then(|body| body.strip_prefix(ION_VERSION))
        .context("invalid DRMION header or trailer")?;
    // Envelope and field names are represented by numerical symbol IDs.
    // Check the shared table import before interpreting those IDs.
    let table = ion::value(&mut document)?;
    ensure!(
        table.kind == 13 && table.annotation == 3 && ion::protected_data_import(table.body)?,
        "unsupported DRMION symbol table"
    );
    let doctype = ion::value(&mut document)?;
    ensure!(
        doctype.kind == 7 && doctype.annotation == DOCTYPE,
        "missing DRMION doctype"
    );

    let mut key = None;
    let mut output = Vec::new();
    let mut page_buffer = Vec::new();
    loop {
        // A stream can end cleanly at a page boundary after losing later pages.
        // Require the format's end marker before publishing any output.
        ensure!(!document.is_empty(), "missing DRMION end marker");
        let envelope = ion::value(&mut document)?;
        if envelope.kind == 7 && envelope.annotation == ENDDOC {
            ensure!(
                document.is_empty(),
                "unexpected data after DRMION end marker"
            );
            break;
        }
        // Indexes describe locations in the wrapped delivery. Sequential
        // reconstruction only needs the pages in their stored order.
        if matches!(envelope.annotation, INDEX_TABLE | INDEX_OFFSET) {
            continue;
        }
        ensure!(
            envelope.kind == 11 && ENVELOPE.contains(&envelope.annotation),
            "unexpected DRMION envelope type"
        );
        let mut entries = envelope.body;
        while !entries.is_empty() {
            let entry = ion::value(&mut entries)?;
            match entry.annotation {
                annotation if METADATA.contains(&annotation) => {
                    // Metadata selects the key for the following pages.
                    // Only inherit the current key when no identifier is declared.
                    key = lookup_key(entry.body, keys, key)?;
                }
                annotation if ENCRYPTED_PAGE.contains(&annotation) => {
                    write_page(
                        entry,
                        Some(key.context("no derived content key matches this voucher")?),
                        &mut page_buffer,
                        &mut output,
                    )?;
                }
                annotation if PLAIN_PAGE.contains(&annotation) => {
                    write_page(entry, None, &mut page_buffer, &mut output)?;
                }
                0 | 23 | INDEX_TABLE => {}
                _ => bail!("unsupported DRMION envelope entry"),
            }
        }
    }
    ensure!(
        output.starts_with(b"CONT"),
        "decrypted payload is not a KFX container"
    );
    Ok(output)
}

// Metadata can refer to an encryption key ID or a voucher ID. When both
// resolve, they must identify the same key.
fn lookup_key(
    mut metadata: &[u8],
    keys: &ContentKeys,
    previous: Option<[u8; 16]>,
) -> Result<Option<[u8; 16]>> {
    let mut matched = None;
    let mut has_reference = false;
    while !metadata.is_empty() {
        let (id, value) = ion::field(&mut metadata)?;
        if !matches!(id, KEY_ID | VOUCHER_ID) {
            continue;
        }
        ensure!(value.kind == 8, "invalid DRMION key reference");
        has_reference = true;
        if let Some(key) = keys.get(std::str::from_utf8(value.body)?) {
            ensure!(
                matched.is_none_or(|existing| existing == *key),
                "conflicting content keys for DRMION component"
            );
            matched = Some(*key);
        }
    }
    Ok(if has_reference { matched } else { previous })
}

fn write_page(
    page: Value<'_>,
    key: Option<[u8; 16]>,
    page_buffer: &mut Vec<u8>,
    output: &mut Vec<u8>,
) -> Result<()> {
    ensure!(page.kind == 13, "invalid DRMION page");
    let mut fields = page.body;
    let mut iv = None;
    let mut payload = None;
    let mut compressed = false;
    while !fields.is_empty() {
        let (id, field) = ion::field(&mut fields)?;
        compressed |= field.annotation == COMPRESSED;
        if field.kind == 10 {
            match id {
                CIPHER_IV => iv = Some(field.body),
                CIPHER_TEXT if key.is_some() => payload = Some(field.body),
                DATA if key.is_none() => payload = Some(field.body),
                _ => {}
            }
        }
    }
    let payload = payload.context("DRMION page has no payload")?;
    // Each encrypted page has its own CBC IV and PKCS#7 padding. Plain
    // pages enter the same reconstruction path without CBC decryption.
    let decoded = if let Some(key) = key {
        let iv = iv
            .and_then(|bytes| bytes.get(..16))
            .context("invalid DRMION page IV")?;
        page_buffer.clear();
        page_buffer.extend_from_slice(payload);
        cbc::Decryptor::<Aes128>::new_from_slices(&key, iv)
            .map_err(|_| anyhow::anyhow!("invalid AES key or IV"))?
            .decrypt_padded_mut::<Pkcs7>(page_buffer)
            .map_err(|_| anyhow::anyhow!("invalid DRMION padding, content key or page"))?
    } else {
        payload
    };

    let mut writer = LimitedContainer(output);
    // Compression happened before encryption, so expand the plaintext
    // after CBC. The leading zero selects unfiltered LZMA-Alone data.
    if let Some(lzma) = decoded.strip_prefix(&[0]).filter(|_| compressed) {
        let options = lzma_rs::decompress::Options {
            memlimit: Some(MAX_CONTAINER),
            ..Default::default()
        };
        lzma_rs::lzma_decompress_with_options(&mut Cursor::new(lzma), &mut writer, &options)
            .context("invalid DRMION LZMA page")?;
    } else {
        ensure!(!compressed, "unsupported DRMION LZMA filter");
        writer.write_all(decoded)?;
    }
    Ok(())
}

// Enforce the output bound as pages are appended, including output from
// the decompressor, so a small input cannot expand without limit.
struct LimitedContainer<'a>(&'a mut Vec<u8>);

impl Write for LimitedContainer<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_CONTAINER - self.0.len() {
            return Err(std::io::Error::other("KFX container exceeds size limit"));
        }
        self.0.write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ContentKeys, lookup_key};

    #[test]
    fn missing_voucher_does_not_reuse_previous_key() {
        let previous = Some([42; 16]);
        let keys = ContentKeys::new();
        assert_eq!(lookup_key(b"", &keys, previous).unwrap(), previous);
        // Field 16 is encryption_voucher, followed by an Ion string.
        assert_eq!(
            lookup_key(b"\x90\x87missing", &keys, previous).unwrap(),
            None
        );
    }
}
