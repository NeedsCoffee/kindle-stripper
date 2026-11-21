//! Kindle MSIX secure storage and registration, using the current Windows account.

use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use hkdf::Hkdf;
use md5::{Digest, Md5};
use serde_json::Value;
use sha1::Sha1;
use sha2::Sha256;
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    ptr, slice,
};
use windows_sys::{
    Win32::{Foundation::LocalFree, Security::Cryptography::*},
    core::w,
};

const PACKAGE: &str = "Packages/AMZNKindle.AmazonKindleReadingApp_m1sc522ngdk36/LocalState";
const ALPHABET: &[u8] = b"YvaZ3FfUm9Nn_c1XuG4yCAzB0beVg-TtHh5SsIiR6rJjQdW2wEq7KkPpL8lOoMxD";

pub fn local_state() -> Result<PathBuf> {
    let local = env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is unavailable")?;
    Ok(Path::new(&local).join(PACKAGE))
}

/// Recovers the client ID and account-token records used by the voucher lock.
/// Based on the MSIX path in DeDRM's kindlekey.py and msix_registration.py.
pub fn credential_database() -> Result<BTreeMap<&'static str, Vec<u8>>> {
    let state = local_state()?;
    let storage = state.join("Classic/Data/storage");
    let blob = fs::read(storage.join("main_shared.blob")).context("reading Kindle storage key")?;
    let salt = fs::read(storage.join("main_shared.salt")).context("reading Kindle storage salt")?;
    let master = unwrap_storage_key(&blob)?;
    // The first HKDF makes a storage-wide key from the CNG plaintext and
    // Kindle's salt. A second HKDF below derives a separate key for each record.
    let storage_key = hkdf(&master, &salt, b"kindle_ss")?;
    let file = fs::read(storage.join(".kinf2024")).context("reading Kindle secure storage")?;
    let separator = file
        .iter()
        .position(|byte| *byte == b'#')
        .context("invalid Kindle storage header")?;
    // The header ends at '#'. Each following '}'-delimited record still
    // needs the reversible text transforms before its AES-GCM data can be read.
    let records: Vec<_> = file[separator + 1..]
        .split(|byte| *byte == b'}')
        .filter(|record| !record.is_empty())
        .map(read_record)
        .collect::<Result<_>>()?;
    ensure!(!records.is_empty(), "Kindle secure storage is empty");
    // The permutation key is the sum of the ASCII digits of a Windows volume serial.
    let permutation = (48..=570)
        .find(|key| {
            records
                .iter()
                .take(4)
                .all(|record| decode_record(record, *key).is_some())
        })
        .context("could not identify the Kindle storage record permutation")?;

    let mut credentials = BTreeMap::new();
    let names = ["DSN", "kindle.account.tokens", "kindle.metrics.checksum"];
    // Kindle stores encoded MD5 digests of field names. Compute the three
    // on-disk identifiers we need and leave other account records encrypted.
    let identifiers = names.map(|name| {
        Md5::digest(name.as_bytes())
            .iter()
            .flat_map(|byte| {
                [
                    ALPHABET[usize::from(byte ^ 128) / 64],
                    ALPHABET[usize::from(*byte) % 64],
                ]
            })
            .collect::<Vec<_>>()
    });
    for record in &records {
        let Some((id, value)) = decode_record(record, permutation) else {
            continue;
        };
        let Some(index) = identifiers.iter().position(|candidate| *candidate == id) else {
            continue;
        };
        let encrypted = decode_alphabet(&value).context("invalid Kindle credential encoding")?;
        ensure!(encrypted.len() >= 28, "truncated Kindle credential");
        // Use SHA-1 of the encoded record name as the entry salt. The
        // stored value is a 12-byte nonce, ciphertext, and a 16-byte tag.
        // The tag checks the key and record integrity before we use plaintext.
        let key = hkdf(&storage_key, &Sha1::digest(&id), b"kindle_ss_token")?;
        let plaintext = Aes256Gcm::new((&key).into())
            .decrypt((&encrypted[..12]).into(), &encrypted[12..])
            .map_err(|_| anyhow::anyhow!("Kindle credential authentication failed"))?;
        credentials.insert(names[index], plaintext);
    }
    // Some versions keep the client ID (DSN) only in Windows registration data.
    if !credentials.contains_key("DSN") {
        credentials.insert("DSN", registration_dsn(&state)?.into_bytes());
    }
    ensure!(
        credentials.contains_key("kindle.account.tokens")
            || credentials.contains_key("kindle.metrics.checksum"),
        "Kindle secure storage contains no supported account credential"
    );
    Ok(credentials)
}

fn hkdf(key: &[u8], salt: &[u8], info: &[u8]) -> Result<[u8; 32]> {
    let mut result = [0; 32];
    Hkdf::<Sha256>::new(Some(salt), key)
        .expand(info, &mut result)
        .map_err(|_| anyhow::anyhow!("Kindle storage key expansion failed"))?;
    Ok(result)
}

struct CryptographyHandle(usize);

impl Drop for CryptographyHandle {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe { NCryptFreeObject(self.0) };
        }
    }
}

fn unwrap_storage_key(blob: &[u8]) -> Result<[u8; 32]> {
    ensure!(blob.len() == 256, "invalid Kindle storage key blob size");
    // The wrapped storage master is RSA-2048 OAEP with SHA-256. Try the
    // TPM-backed provider first, then the software provider used by some installs.
    let padding = BCRYPT_OAEP_PADDING_INFO {
        pszAlgId: w!("SHA256"),
        pbLabel: ptr::null_mut(),
        cbLabel: 0,
    };
    for name in [
        w!("Microsoft Platform Crypto Provider"),
        w!("Microsoft Software Key Storage Provider"),
    ] {
        let mut provider = CryptographyHandle(0);
        let mut key = CryptographyHandle(0);
        let mut plaintext = [0; 256];
        let mut length = 0;
        // The key remains in the Windows provider. Only the wrapped Kindle storage key is decrypted.
        unsafe {
            if NCryptOpenStorageProvider(&mut provider.0, name, 0) != 0
                || NCryptOpenKey(
                    provider.0,
                    &mut key.0,
                    w!("KindleForPC_SS"),
                    0,
                    NCRYPT_SILENT_FLAG,
                ) != 0
            {
                continue;
            }
            if NCryptDecrypt(
                key.0,
                blob.as_ptr(),
                blob.len() as u32,
                ptr::from_ref(&padding).cast(),
                plaintext.as_mut_ptr(),
                plaintext.len() as u32,
                &mut length,
                NCRYPT_PAD_OAEP_FLAG | NCRYPT_SILENT_FLAG,
            ) == 0
                && length >= 32
            {
                // The first 32 bytes are the master key. The rest is padding
                // inside the RSA plaintext and does not enter the HKDF.
                return Ok(plaintext[..32].try_into()?);
            }
        }
    }
    bail!("Windows could not open the Kindle storage key for this account")
}

// registration_data is a Base64-encoded DPAPI blob containing JSON.
// Windows opens it in the current user's context with prompts disabled.
fn registration_dsn(state: &Path) -> Result<String> {
    let encoded =
        fs::read(state.join("registration_data")).context("reading Kindle registration")?;
    ensure!(
        encoded.len() <= 1024 * 1024,
        "invalid Kindle registration size"
    );
    let mut encrypted = STANDARD.decode(
        encoded
            .into_iter()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect::<Vec<_>>(),
    )?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: encrypted.len() as u32,
        pbData: encrypted.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let decoded = unsafe {
        ensure!(
            CryptUnprotectData(
                &input,
                ptr::null_mut(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output
            ) != 0,
            "Windows could not unlock Kindle registration"
        );
        ensure!(
            !output.pbData.is_null(),
            "Windows returned empty Kindle registration"
        );
        let bytes = slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
        let result =
            serde_json::from_slice::<Value>(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes));
        bytes.fill(0);
        LocalFree(output.pbData.cast());
        result
    }?;
    let dsn = decoded["dsn"]
        .as_str()
        .filter(|value| value.is_ascii() && matches!(value.len(), 16 | 32 | 40))
        .context("invalid Kindle registration client ID")?;
    Ok(dsn.to_owned())
}

// Undo the first two storage transforms. Reversing the record and removing
// its marker exposes alternating Caesar shifts over the Base64 alphabet's
// letter and digit ranges. Decoding gives comma-separated LZW code numbers.
fn read_record(record: &[u8]) -> Result<Vec<u8>> {
    let mut base64: Vec<_> = record
        .iter()
        .rev()
        .skip(1)
        .enumerate()
        .map(|(index, byte)| {
            let (start, size) = match byte {
                b'A'..=b'Z' => (b'A', 26),
                b'a'..=b'z' => (b'a', 26),
                b'0'..=b'9' => (b'0', 10),
                _ => return *byte,
            };
            let shift = if index % 2 == 0 { 1 } else { size - 1 };
            start + (byte - start + shift) % size
        })
        .collect();
    while base64.len() % 4 != 0 {
        base64.push(b'=');
    }
    let codes = STANDARD.decode(base64)?;
    let codes = std::str::from_utf8(&codes)?
        .split(',')
        .filter(|code| !code.is_empty());
    let mut dictionary: Vec<_> = (0..=255).map(|byte| vec![byte]).collect();
    let mut previous = Vec::new();
    let mut decoded = Vec::new();
    for code in codes {
        let index: usize = code.parse()?;
        let entry = match dictionary.get(index) {
            Some(entry) => entry.clone(),
            None if index == dictionary.len() && !previous.is_empty() => {
                // LZW can reference the dictionary entry being created:
                // it is the previous phrase plus its own first byte.
                let mut entry = previous.clone();
                entry.push(previous[0]);
                entry
            }
            None => bail!("invalid Kindle storage record code"),
        };
        decoded.extend_from_slice(&entry);
        if !previous.is_empty() {
            previous.push(entry[0]);
            dictionary.push(previous);
        }
        previous = entry;
    }
    Ok(decoded)
}

// Undo the device-specific permutation, expand bit runs, and transpose the
// five-row grid back into the encoded name and value.
fn decode_record(record: &[u8], key: usize) -> Option<(Vec<u8>, Vec<u8>)> {
    let span = key - 1;
    let mut positions: Vec<_> = (0..record.len()).collect();
    // The file stores bytes ordered by a triangular bounce with the original
    // index as the tie-breaker. Invert that ordering to recover the run stream.
    positions.sort_unstable_by_key(|index| {
        let phase = index % (2 * span);
        (phase.min(2 * span - phase), *index)
    });
    let mut permuted = vec![0; record.len()];
    for (source, destination) in positions.into_iter().enumerate() {
        permuted[destination] = record[source];
    }
    let mut bits = Vec::new();
    // Z separates runs. Letters a through f encode lengths 1 through 6.
    // A splits a zero run from a one run, or marks a final zero-only run.
    let mut tokens = permuted.split(|byte| *byte == b'Z').peekable();
    while let Some(token) = tokens.next() {
        let run = |value| {
            (b'a'..=b'f')
                .contains(&value)
                .then(|| usize::from(value - b'a') + 1)
        };
        match token {
            [] => {}
            [zero, b'A', one] => {
                bits.extend(std::iter::repeat_n(0, run(*zero)?));
                bits.extend(std::iter::repeat_n(1, run(*one)?));
            }
            [zero, b'A'] if tokens.peek().is_none() => {
                bits.extend(std::iter::repeat_n(0, run(*zero)?))
            }
            _ => bits.extend(std::iter::repeat_n(1, run(*token.last()?)?)),
        }
    }
    if bits.len() % 8 != 0 || bits.is_empty() {
        return None;
    }
    // The bit rotation is derived from the same volume serial as the
    // permutation. Undo it before packing the stream into eight-bit bytes.
    bits.rotate_right(key % 5);
    let rows: Vec<u8> = bits
        .as_chunks::<8>()
        .0
        .iter()
        .map(|byte| byte.iter().fold(0, |sum, bit| (sum << 1) | bit))
        .collect();
    let columns = rows.iter().filter(|byte| **byte != b'^').count() / 5 + 1;
    if rows.len() != columns * 5 {
        return None;
    }
    // The grid was stored by row. Read it by column, restore the space
    // placeholders, then split the 32-byte encoded name from its value.
    let mut flat: Vec<_> = (0..columns)
        .flat_map(|column| (0..5).map(move |row| row * columns + column))
        .map(|index| {
            if rows[index] == b'^' {
                b' '
            } else {
                rows[index]
            }
        })
        .collect();
    flat.truncate(flat.iter().rposition(|byte| *byte != b' ')? + 1);
    if flat.get(32) != Some(&b'*') || flat.len() <= 33 {
        return None;
    }
    let value = flat.split_off(33);
    flat.pop();
    (flat
        .iter()
        .chain(&value)
        .all(|byte| ALPHABET.contains(byte)))
    .then_some((flat, value))
}

// The storage alphabet encodes a byte as two symbols. The high symbol
// carries the byte's high bits after XOR with 0x80.
fn decode_alphabet(value: &[u8]) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let high = ALPHABET.iter().position(|byte| *byte == pair[0])?;
            let low = ALPHABET.iter().position(|byte| *byte == pair[1])?;
            u8::try_from(((high * 64) ^ 128) + low).ok()
        })
        .collect()
}
