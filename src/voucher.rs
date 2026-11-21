//! Native derivation for the desktop selector carried by the Kindle MSIX downloads.

use crate::{
    drmion::ContentKeys,
    ion::{self, Value},
    voucher_mixer,
};
use aes::Aes256;
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use cbc::cipher::{
    BlockDecryptMut, KeyIvInit,
    block_padding::{NoPadding, Pkcs7},
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::{collections::BTreeMap, fs, io::Read, path::Path};

const ION_MARKER: &[u8] = b"\xe0\x01\x00\xea";
// Supported desktop vouchers carry a sub-kilobyte ciphertext. Leave room for
// envelope metadata while bounding the read even if the file grows.
const MAX_VOUCHER_BYTES: u64 = 64 * 1024;
// Fixed format constants for desktop selector 0x8a from DeDRM's
// voucher_route.py and ion.py. The user-specific inputs come from Kindle storage.
const ISSUANCE_KEY: &[u8; 32] = b"x2pLr0KQPuJQzOWbeENvbG4z1ENj6WT1";
const WRAPPED_KEY: &[u8; 32] = b"DR1DvpN7nMnvPIzW8VEjZZE4A9SQWn0g";
const ACCOUNT_WRAP_KEYS: [&[u8; 32]; 3] = [
    b"e35f5062f97cc8b1244f6f1a2414e31c",
    b"ZK9WCSU44U6JTOr0FJQtMBue7XFxmTrY",
    b"dh2D9VlAwoSXF6r78VdaJsr79Kls8Hg3",
];

/// Derive the AES-128 page key for each voucher, indexed by the ID referenced
/// by the corresponding DRMION component.
pub fn derive_keys(
    folder: &Path,
    credentials: &BTreeMap<&str, Vec<u8>>,
    verbose: bool,
) -> Result<ContentKeys> {
    let clients = credentials.get("DSN").map_or(&[][..], std::slice::from_ref);
    let secrets = ["kindle.account.tokens", "kindle.metrics.checksum"]
        .into_iter()
        .filter_map(|name| credentials.get(name))
        .flat_map(|value| account_secrets(value))
        .collect::<Vec<_>>();
    let mut keys = ContentKeys::new();
    for file in fs::read_dir(folder)? {
        let file = file?;
        let path = file.path();
        if !file.file_type()?.is_file()
            || !path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("voucher"))
        {
            continue;
        }
        let mut data = Vec::new();
        fs::File::open(&path)?
            .take(MAX_VOUCHER_BYTES + 1)
            .read_to_end(&mut data)?;
        ensure!(
            data.len() as u64 <= MAX_VOUCHER_BYTES,
            "{} exceeds voucher size limit",
            crate::windows_paths::display(&path)
        );
        let voucher = Voucher::parse(&data)
            .with_context(|| format!("reading {}", crate::windows_paths::display(&path)))?;
        let key = voucher.derive(clients, &secrets).with_context(|| {
            format!(
                "deriving content key for {}",
                crate::windows_paths::display(&path)
            )
        })?;
        if verbose {
            eprintln!(
                "Derived content key for {:?} (selector 0x8A, account lock: {}, client lock: {}).",
                voucher.id, voucher.account, voucher.client,
            );
        }
        if let Some(existing) = keys.insert(voucher.id.to_owned(), key) {
            ensure!(
                existing == key,
                "conflicting vouchers for the same identifier"
            );
        }
    }
    ensure!(
        !keys.is_empty(),
        "no voucher found alongside the Kindle book"
    );
    Ok(keys)
}

// Account-token records may contain several candidates, each plain or
// wrapped. Binary tokens stay whole because ciphertext can contain delimiters.
fn account_secrets(value: &[u8]) -> Vec<Vec<u8>> {
    let mut result = Vec::new();
    let printable = |bytes: &[u8]| bytes.iter().all(|byte| matches!(byte, b' '..=b'~'));
    let tokens: Vec<_> = if value
        .iter()
        .all(|byte| matches!(byte, b' '..=b'~' | b'\t' | b'\r' | b'\n'))
    {
        value
            .split(|byte| byte.is_ascii_whitespace() || matches!(byte, b',' | b';'))
            .collect()
    } else {
        vec![value]
    };
    for token in tokens {
        let mut candidate = token.to_vec();
        while !candidate.is_empty() {
            if matches!(candidate.len(), 30 | 40)
                && printable(&candidate)
                && !result.contains(&candidate)
            {
                result.push(candidate.clone());
            }
            // A wrapped value is a 16-byte IV followed by AES-256-CBC
            // ciphertext. Check both PKCS#7 padding and the expected text
            // shape. The voucher keyset provides the later correctness check.
            if candidate.len() >= 32 && candidate.len().is_multiple_of(16) {
                for key in ACCOUNT_WRAP_KEYS {
                    let mut body = candidate[16..].to_vec();
                    let cipher = cbc::Decryptor::<Aes256>::new(key.into(), candidate[..16].into());
                    if let Ok(plaintext) = cipher.decrypt_padded_mut::<Pkcs7>(&mut body)
                        && matches!(plaintext.len(), 30 | 40)
                        && printable(plaintext)
                        && !result.iter().any(|secret| secret == plaintext)
                    {
                        result.push(plaintext.to_vec());
                    }
                }
            }
            // Each Base64 layer is shorter than its input. Continue until
            // we reach a raw token or the current bytes are not Base64.
            let Ok(decoded) = STANDARD.decode(candidate) else {
                break;
            };
            candidate = decoded;
        }
    }
    result
}

struct Voucher<'a> {
    id: &'a str,
    iv: &'a [u8],
    ciphertext: &'a [u8],
    issuance: &'a [u8],
    account: bool,
    client: bool,
}

impl<'a> Voucher<'a> {
    fn parse(data: &'a [u8]) -> Result<Self> {
        // The outer Ion struct selects the algorithm and carries the nested
        // Ion voucher document as a blob.
        let envelope = root(data)?;
        ensure!(envelope.kind == 13, "invalid voucher envelope");
        ensure!(
            envelope.annotation == 0x8a,
            "unsupported voucher selector 0x{:x}",
            envelope.annotation
        );
        let document = root(required_field(envelope.body, 0x41)?.body)?;
        ensure!(document.kind == 13, "invalid wrapped voucher document");
        // Use the outer strategy's lock list when present. Some vouchers
        // carry it only inside the nested document, and an empty list needs
        // neither account secret nor client ID.
        let outer_lock = find_field(envelope.body, 0x24)?
            .map(|strategy| find_field(strategy.body, 0x23))
            .transpose()?
            .flatten();
        let (mut account, mut client) = (false, false);
        if let Some(lock) = outer_lock
            .filter(|lock| !lock.body.is_empty())
            .or(find_field(document.body, 0x23)?)
        {
            let mut fields = lock.body;
            while !fields.is_empty() {
                let field = ion::value(&mut fields)?;
                match field.body {
                    b"ACCOUNT_SECRET" => account = true,
                    b"CLIENT_ID" => client = true,
                    _ => bail!("unsupported voucher lock parameter"),
                }
            }
        }
        let id = std::str::from_utf8(required_field(document.body, 0x22)?.body)?;
        let ciphertext = required_field(document.body, 0x15)?.body;
        let iv = required_field(document.body, 0x16)?.body;
        let issuance = required_field(document.body, 0x78)?.body;
        ensure!(
            matches!(ciphertext.len(), 512 | 544) && iv.len() == 16 && issuance.len() == 64,
            "invalid desktop voucher operands"
        );
        ensure!(
            id.starts_with("amzn1.drm-voucher.v1."),
            "invalid voucher ID"
        );
        Ok(Self {
            id,
            iv,
            ciphertext,
            issuance,
            account,
            client,
        })
    }

    fn derive(&self, clients: &[Vec<u8>], secrets: &[Vec<u8>]) -> Result<[u8; 16]> {
        let clients = lock_candidates(clients, self.client, &[16, 32, 40]);
        let secrets = lock_candidates(secrets, self.account, &[30, 40]);
        // The issuance record starts with an IV and two ciphertext blocks.
        // Opening those blocks under the selector's fixed key gives a 32-byte
        // mask that is independent of the local credential candidates.
        let mask = decrypt(ISSUANCE_KEY, &self.issuance[..16], &self.issuance[16..48])?;
        for client in clients {
            for secret in &secrets {
                let vector = voucher_mixer::derive_vector(*secret, client, self.id.as_bytes());
                // The selector's mixer yields the HMAC key. HMAC-SHA256 over
                // the fixed PIDv3 message produces the wrapping-key material.
                let mut mac = Hmac::<Sha256>::new_from_slice(&vector)
                    .map_err(|_| anyhow::anyhow!("could not initialize voucher HMAC"))?;
                mac.update(b"PIDv3");
                let mut key: [u8; 32] = mac.finalize().into_bytes().into();
                // XOR in the issuance mask before opening the voucher with
                // AES-256-CBC. A candidate must produce a usable Ion keyset.
                for (slot, mask) in key.iter_mut().zip(&mask) {
                    *slot ^= mask;
                }
                let plain = decrypt(&key, self.iv, self.ciphertext)?;
                if let Ok(key) = content_key(&plain, self.id.as_bytes()) {
                    return Ok(key);
                }
            }
        }
        bail!("no Kindle credential combination opened this voucher")
    }
}

// Supply no value for an absent lock. For a declared lock, filter the local
// candidates to the text lengths accepted by the desktop voucher format.
fn lock_candidates<'a>(
    values: &'a [Vec<u8>],
    required: bool,
    lengths: &[usize],
) -> Vec<Option<&'a [u8]>> {
    if !required {
        return vec![None];
    }
    values
        .iter()
        .filter(|value| lengths.contains(&value.len()) && value.is_ascii())
        .map(|value| Some(value.as_slice()))
        .collect()
}

fn decrypt(key: &[u8; 32], iv: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
    let mut bytes = ciphertext.to_vec();
    cbc::Decryptor::<Aes256>::new_from_slices(key, iv)
        .map_err(|_| anyhow::anyhow!("invalid voucher IV"))?
        .decrypt_padded_mut::<NoPadding>(&mut bytes)
        .map_err(|_| anyhow::anyhow!("invalid voucher ciphertext alignment"))?;
    Ok(bytes)
}

// A keyset can carry the AES-128 key directly or in an inner wrapped
// key record. The inner format binds its plaintext to the voucher ID.
fn content_key(data: &[u8], id: &[u8]) -> Result<[u8; 16]> {
    let keyset = root(data)?;
    ensure!(
        keyset.annotation == 0x26,
        "voucher did not produce an Ion keyset"
    );
    let encoded = find_encoded_key(keyset, 0)?.context("missing encoded content key")?;
    match encoded.len() {
        16 => Ok(encoded.try_into()?),
        48 => {
            // The first block is the inner IV. Decrypt the next block, then
            // XOR with the first 16 bytes of the voucher ID to obtain the key.
            let bytes = decrypt(WRAPPED_KEY, &encoded[..16], &encoded[16..32])?;
            Ok(std::array::from_fn(|index| bytes[index] ^ id[index]))
        }
        _ => bail!("invalid content key length"),
    }
}

fn find_encoded_key(item: Value<'_>, depth: usize) -> Result<Option<&[u8]>> {
    ensure!(depth < 16, "Ion keyset nesting limit exceeded");
    if item.kind == 13
        && let Some(blob) = find_field(item.body, 0x1d)?
        && blob.kind == 10
        && matches!(blob.body.len(), 16 | 48)
    {
        return Ok(Some(blob.body));
    }
    let mut children = item.body;
    while !children.is_empty() {
        let child = match item.kind {
            13 => ion::field(&mut children)?.1,
            11 | 12 => ion::value(&mut children)?,
            _ => break,
        };
        if let Some(encoded) = find_encoded_key(child, depth + 1)? {
            return Ok(Some(encoded));
        }
    }
    Ok(None)
}

// An Ion version marker starts each document. Skip leading padding and
// the local symbol table to reach the first annotated container.
fn root(data: &[u8]) -> Result<Value<'_>> {
    let mut data = data
        .strip_prefix(ION_MARKER)
        .context("missing voucher Ion header")?;
    while !data.is_empty() {
        let value = ion::value(&mut data)?;
        if matches!(value.kind, 11 | 13) && value.annotation != 3 {
            return Ok(value);
        }
        ensure!(
            value.kind == 0 || value.annotation == 3,
            "unexpected voucher Ion value"
        );
    }
    bail!("empty voucher Ion document")
}

fn find_field(mut input: &[u8], id: usize) -> Result<Option<Value<'_>>> {
    while !input.is_empty() {
        let (field, value) = ion::field(&mut input)?;
        if field == id {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn required_field(input: &[u8], id: usize) -> Result<Value<'_>> {
    find_field(input, id)?.with_context(|| format!("missing voucher Ion field 0x{id:x}"))
}

#[cfg(test)]
mod tests {
    use super::{BTreeMap, Voucher, derive_keys, fs};
    use crate::parser_fuzz::{annotated, exercise, field, value};

    #[test]
    fn oversized_voucher_is_rejected_before_parsing() {
        let folder = tempfile::tempdir().unwrap();
        let file = fs::File::create(folder.path().join("oversized.voucher")).unwrap();
        file.set_len(64 * 1024 + 1).unwrap();
        let error = derive_keys(folder.path(), &BTreeMap::new(), false).unwrap_err();
        assert!(error.to_string().contains("exceeds voucher size limit"));
    }

    #[test]
    fn fuzz_voucher_parser() {
        let mut nested = b"\xe0\x01\x00\xea".to_vec();
        let fields = [
            field(0x22, 8, b"amzn1.drm-voucher.v1.synthetic"),
            field(0x15, 10, &[0; 544]),
            field(0x16, 10, &[0; 16]),
            field(0x78, 10, &[0; 64]),
        ]
        .concat();
        nested.extend(value(13, &fields));
        let mut seed = b"\xe0\x01\x00\xea".to_vec();
        seed.extend(annotated(0x8a, &value(13, &field(0x41, 10, &nested))));
        assert!(Voucher::parse(&seed).is_ok());
        exercise(&seed, |input| {
            let _ = Voucher::parse(input);
        });
    }
}
