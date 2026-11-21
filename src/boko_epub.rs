use crate::{
    epub::entries,
    kfx_pdf::{list, number, record_value, required},
};
use anyhow::{Context, Result, ensure};
use boko::kfx::container::{parse_container_header, parse_container_info, parse_index_table};
use std::{
    collections::HashMap,
    io::{Seek, Write},
};

const HEADER_SIZE: usize = 18;
const MAX_BOOK_SIZE: usize = 256 * 1024 * 1024;

#[derive(Default)]
pub struct MultipartBook {
    index: Vec<u8>,
    payload: Vec<u8>,
    symbols: Vec<u8>,
    ids: HashMap<(u32, u32), (usize, usize)>,
}

fn region(bytes: &[u8], offset: usize, length: usize) -> Result<&[u8]> {
    let end = offset.checked_add(length).context("KFX range overflow")?;
    bytes
        .get(offset..end)
        .context("KFX range outside its container")
}

impl MultipartBook {
    pub fn add(&mut self, component: &[u8]) -> Result<()> {
        let header = parse_container_header(component)?;
        let header_bytes = region(component, 0, header.header_len)?;
        let info = parse_container_info(region(
            header_bytes,
            header.container_info_offset,
            header.container_info_length,
        )?)?;
        if let Some((offset, length)) = info.doc_symbols.filter(|&(_, length)| length > 0) {
            let symbols = region(header_bytes, offset, length)?;
            ensure!(
                symbols.len() <= 16 * 1024 * 1024,
                "boko symbol table exceeds size limit"
            );
            if self.symbols.is_empty() {
                self.symbols.extend_from_slice(symbols);
            } else {
                ensure!(
                    self.symbols == symbols,
                    "boko requires a shared KFX symbol table"
                );
            }
        }
        let (offset, length) = info.index.context("missing KFX entity index")?;
        let index = region(header_bytes, offset, length)?;
        ensure!(
            length.is_multiple_of(24) && self.index.len() / 24 + length / 24 <= 100_000,
            "invalid or oversized KFX entity index"
        );
        for entry in parse_index_table(index, header.header_len) {
            let bytes = region(component, entry.offset, entry.length)?;
            // KFX allows multiple unnamed position-map fragments (262:348).
            // Compare other repeated IDs byte-for-byte before deduplicating.
            if (entry.type_id, entry.id) != (262, 348) {
                let key = (entry.type_id, entry.id);
                if let Some(&(offset, length)) = self.ids.get(&key) {
                    ensure!(
                        region(&self.payload, offset, length)? == bytes,
                        "conflicting KFX entity {}:{} in multipart book",
                        entry.type_id,
                        entry.id
                    );
                    continue;
                }
                self.ids.insert(key, (self.payload.len(), bytes.len()));
            }
            ensure!(
                self.payload.len() + bytes.len() <= MAX_BOOK_SIZE,
                "boko input exceeds size limit"
            );
            self.index.extend_from_slice(&entry.id.to_le_bytes());
            self.index.extend_from_slice(&entry.type_id.to_le_bytes());
            self.index
                .extend_from_slice(&(self.payload.len() as u64).to_le_bytes());
            self.index
                .extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            self.payload.extend_from_slice(bytes);
        }
        Ok(())
    }

    fn rebase_locals(&mut self, first_local: usize) -> Result<()> {
        let base = boko::kfx::symbols::KFX_SYMBOL_TABLE.len();
        let old_payload = std::mem::take(&mut self.payload);
        for (entry, index) in parse_index_table(&self.index, 0)
            .into_iter()
            .zip(self.index.chunks_exact_mut(24))
        {
            let bytes = region(&old_payload, entry.offset, entry.length)?;
            let start = self.payload.len();
            if matches!(entry.type_id, 417 | 418) {
                self.payload.extend_from_slice(bytes);
            } else {
                ensure!(bytes.starts_with(b"ENTY"), "missing KFX entity header");
                let header_length =
                    usize::try_from(u32::from_le_bytes(region(bytes, 6, 4)?.try_into()?))?;
                ensure!(header_length >= 10, "invalid KFX entity header");
                self.payload
                    .extend_from_slice(region(bytes, 0, header_length)?);
                let mut ion = &bytes[header_length..];
                if let Some(rest) = ion.strip_prefix(b"\xe0\x01\x00\xea") {
                    self.payload.extend_from_slice(&ion[..4]);
                    ion = rest;
                }
                while !ion.is_empty() {
                    rebase_ion(
                        &mut ion,
                        first_local,
                        base - first_local,
                        0,
                        &mut self.payload,
                    )?;
                }
            }
            ensure!(
                self.payload.len() <= MAX_BOOK_SIZE,
                "boko input exceeds size limit"
            );
            let id = shift(entry.id as usize, first_local, base - first_local)?;
            index[..4].copy_from_slice(&u32::try_from(id)?.to_le_bytes());
            index[8..16].copy_from_slice(&(start as u64).to_le_bytes());
            index[16..24].copy_from_slice(&((self.payload.len() - start) as u64).to_le_bytes());
        }
        Ok(())
    }

    pub fn write(mut self, output: &mut (impl Write + Seek)) -> Result<()> {
        // boko reads a single CONT container. The synthetic index points at
        // the original entities under the book-wide document symbol table.
        let (symbols, first_local) = align_symbols(&self.symbols)?;
        if let Some(first_local) = first_local {
            self.rebase_locals(first_local)?;
        }
        let symbols_offset = HEADER_SIZE + self.index.len();
        let info_offset = symbols_offset + symbols.len();
        let mut fields = Vec::new();
        for (id, value) in [
            (413u16, HEADER_SIZE),
            (414, self.index.len()),
            (415, symbols_offset),
            (416, symbols.len()),
        ] {
            fields.extend([(id >> 7) as u8, (id as u8 & 127) | 128, 0x24]);
            fields.extend(u32::try_from(value)?.to_be_bytes());
        }
        let mut info = vec![
            0xe0,
            0x01,
            0x00,
            0xea,
            0xde,
            u8::try_from(fields.len())? | 128,
        ];
        info.extend(fields);
        let header_size = info_offset + info.len();
        let mut input = Vec::with_capacity(header_size + self.payload.len());
        input.extend(b"CONT\x02\0");
        input.extend(u32::try_from(header_size)?.to_le_bytes());
        input.extend(u32::try_from(info_offset)?.to_le_bytes());
        input.extend(u32::try_from(info.len())?.to_le_bytes());
        input.extend(self.index);
        input.extend(symbols);
        input.extend(info);
        input.extend(self.payload);
        let book = boko::Book::from_bytes(&input, boko::Format::Kfx)
            .context("opening multipart KFX with boko")?;
        drop(input);
        ensure!(!book.spine().is_empty(), "boko found no readable sections");
        book.export(boko::Format::Epub, output)
            .context("writing EPUB with boko")
    }
}

fn varuint(mut value: usize, bytes: &mut Vec<u8>) {
    let mut groups = [0u8; 10];
    let mut start = groups.len() - 1;
    groups[start] = (value as u8 & 127) | 128;
    value >>= 7;
    while value != 0 {
        start -= 1;
        groups[start] = value as u8 & 127;
        value >>= 7;
    }
    bytes.extend_from_slice(&groups[start..]);
}

fn ion_value(kind: u8, body: &[u8], bytes: &mut Vec<u8>) {
    if body.len() < 14 {
        bytes.push(kind << 4 | body.len() as u8);
    } else {
        bytes.push(kind << 4 | 14);
        varuint(body.len(), bytes);
    }
    bytes.extend_from_slice(body);
}

fn align_symbols(symbols: &[u8]) -> Result<(Vec<u8>, Option<usize>)> {
    if symbols.is_empty() {
        return Ok((Vec::new(), None));
    }
    let table = record_value(symbols)?;
    let mut imports = entries(list(required(table.body, 6)?)?);
    let import = imports.next().context("missing KFX symbol import")??;
    ensure!(
        imports.next().is_none() && required(import.body, 4)?.body == b"YJ_symbols",
        "unsupported boko symbol imports"
    );
    let first_local = usize::try_from(number(required(import.body, 8)?, 2)?)?
        .checked_add(1)
        .context("KFX symbol ID overflow")?;
    // boko indexes its local names immediately after its fixed shared table.
    // Real Kindle downloads can declare a different shared table length.
    let boko_first_local = boko::kfx::symbols::KFX_SYMBOL_TABLE.len();
    let padding = first_local.saturating_sub(boko_first_local);
    ensure!(padding <= 4096, "boko symbol table offset exceeds limit");
    let mut local_names = vec![0x80; padding];
    for value in entries(list(required(table.body, 7)?)?) {
        let value = value?;
        let name = if value.kind == 8 { value.body } else { &[] };
        ion_value(8, name, &mut local_names);
    }
    let mut fields = vec![0x87]; // Ion field 7 contains the local symbol names.
    ion_value(11, &local_names, &mut fields);
    let mut annotated = vec![0x81, 0x83]; // $ion_symbol_table annotation.
    ion_value(13, &fields, &mut annotated);
    let mut encoded = vec![0xe0, 1, 0, 0xea];
    ion_value(14, &annotated, &mut encoded);
    Ok((
        encoded,
        (first_local < boko_first_local).then_some(first_local),
    ))
}

fn shift(id: usize, first_local: usize, delta: usize) -> Result<usize> {
    if id < first_local {
        Ok(id)
    } else {
        id.checked_add(delta).context("KFX symbol ID overflow")
    }
}

fn read_varuint(bytes: &mut &[u8]) -> Result<usize> {
    let mut value = 0usize;
    loop {
        let (&byte, rest) = bytes.split_first().context("truncated Ion VarUInt")?;
        *bytes = rest;
        value = value.checked_mul(128).context("Ion VarUInt overflow")? | usize::from(byte & 127);
        if byte & 128 != 0 {
            return Ok(value);
        }
    }
}

// Move document-local SIDs out of the range boko reserves for shared symbols.
// Only Ion symbols, field names and annotations carry SIDs. Integers, blobs
// and media bytes retain their original values.
fn rebase_ion(
    input: &mut &[u8],
    first_local: usize,
    delta: usize,
    depth: usize,
    output: &mut Vec<u8>,
) -> Result<()> {
    ensure!(
        depth < 64 && output.len() <= MAX_BOOK_SIZE,
        "KFX Ion exceeds conversion limits"
    );
    let original = *input;
    let (&descriptor, tail) = input.split_first().context("truncated Ion value")?;
    *input = tail;
    let kind = descriptor >> 4;
    let nibble = descriptor & 15;
    if kind == 1 || nibble == 15 {
        output.push(descriptor);
        return Ok(());
    }
    let size = if nibble == 14 || (kind == 13 && nibble == 1) {
        read_varuint(input)?
    } else {
        usize::from(nibble)
    };
    let (mut body, rest) = input.split_at_checked(size).context("truncated Ion body")?;
    *input = rest;
    let mut rewritten = Vec::new();
    match kind {
        7 => {
            ensure!(body.len() <= size_of::<usize>(), "KFX symbol ID overflow");
            let id = body
                .iter()
                .fold(0usize, |id, &byte| (id << 8) | usize::from(byte));
            let bytes = shift(id, first_local, delta)?.to_be_bytes();
            let start = bytes
                .iter()
                .position(|&byte| byte != 0)
                .unwrap_or(bytes.len());
            rewritten.extend_from_slice(&bytes[start..]);
        }
        11..=13 => {
            while !body.is_empty() {
                if kind == 13 {
                    let id = shift(read_varuint(&mut body)?, first_local, delta)?;
                    varuint(id, &mut rewritten);
                }
                rebase_ion(&mut body, first_local, delta, depth + 1, &mut rewritten)?;
            }
        }
        14 => {
            let size = read_varuint(&mut body)?;
            let (mut annotations, mut value) = body
                .split_at_checked(size)
                .context("truncated Ion annotations")?;
            let mut ids = Vec::new();
            while !annotations.is_empty() {
                let id = shift(read_varuint(&mut annotations)?, first_local, delta)?;
                varuint(id, &mut ids);
            }
            varuint(ids.len(), &mut rewritten);
            rewritten.extend(ids);
            rebase_ion(&mut value, first_local, delta, depth + 1, &mut rewritten)?;
            ensure!(value.is_empty(), "unexpected trailing Ion annotation bytes");
        }
        _ => {
            output.extend_from_slice(&original[..original.len() - rest.len()]);
            return Ok(());
        }
    }
    if kind == 13 && rewritten.len() == 1 {
        output.extend([0xde, 0x81, rewritten[0]]);
    } else {
        ion_value(kind, &rewritten, output);
    }
    Ok(())
}
