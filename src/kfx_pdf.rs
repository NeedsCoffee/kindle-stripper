use crate::ion;
use anyhow::{Context, Result, ensure};

fn region(data: &[u8], start: u64, length: u64) -> Result<&[u8]> {
    let end = start.checked_add(length).context("KFX offset overflow")?;
    data.get(usize::try_from(start)?..usize::try_from(end)?)
        .context("KFX range outside component")
}

fn little_endian(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .rev()
        .fold(0, |value, &byte| (value << 8) | u64::from(byte))
}

type FragmentKey = (u64, u64);

#[derive(Default)]
pub struct Book {
    pub(crate) fragments: std::collections::HashMap<FragmentKey, Vec<u8>>,
    pub(crate) media_names: std::collections::HashMap<String, u64>,
    pdf_ids: Vec<u64>,
    stored_bytes: usize,
}

impl Book {
    pub fn add_component(&mut self, component: &[u8]) -> Result<()> {
        self.read_component(component, false)
    }

    pub fn add_metadata_component(&mut self, component: &[u8]) -> Result<()> {
        self.read_component(component, true)
    }

    fn read_component(&mut self, component: &[u8], metadata_only: bool) -> Result<()> {
        let header = region(component, 0, 18)?;
        ensure!(
            &header[..4] == b"CONT" && matches!(little_endian(&header[4..6]), 1 | 2),
            "unsupported KFX container"
        );
        let header_size = little_endian(&header[6..10]);
        ensure!(header_size >= 18, "invalid KFX header size");
        let header_data = region(component, 0, header_size)?;
        let info = region(
            header_data,
            little_endian(&header[10..14]),
            little_endian(&header[14..18]),
        )?;
        let mut info = info.strip_prefix(b"\xe0\x01\x00\xea").unwrap_or(info);
        let record = ion::value(&mut info)?;
        ensure!(
            record.kind == 13 && info.is_empty(),
            "invalid KFX container info"
        );
        let mut fields = record.body;
        let mut locations = [None; 4];
        while !fields.is_empty() {
            let (id, field) = ion::field(&mut fields)?;
            match id {
                410 | 411 => ensure!(
                    field.kind == 2 && field.body.is_empty(),
                    "unsupported KFX container encoding"
                ),
                413..=416 => {
                    ensure!(
                        field.kind == 2 && field.body.len() <= 8,
                        "invalid KFX index field"
                    );
                    let slot = &mut locations[id - 413];
                    ensure!(
                        slot.replace(number(field, 2)?).is_none(),
                        "duplicate KFX index field"
                    );
                }
                _ => {}
            }
        }
        let [index_offset, index_size, symbols_offset, symbols_size] = locations;
        if let Some(size) = symbols_size.filter(|size| *size != 0 && !metadata_only) {
            ensure!(
                size <= 16 * 1024 * 1024,
                "KFX symbol table exceeds conversion limits"
            );
            let symbols = record_value(region(
                header_data,
                symbols_offset.context("missing KFX symbol offset")?,
                size,
            )?)?;
            let imports = required(symbols.body, 6)?;
            let import = ion::value(&mut list(imports)?)?;
            ensure!(
                required(import.body, 4)?.body == b"YJ_symbols",
                "unsupported KFX symbol import"
            );
            // KFX includes the nine Ion system symbols in its imported maximum.
            let base = number(required(import.body, 8)?, 2)?;
            let mut entries = list(required(symbols.body, 7)?)?;
            let mut id = base;
            while !entries.is_empty() {
                id = id.checked_add(1).context("KFX symbol overflow")?;
                let symbol = ion::value(&mut entries)?;
                if symbol.kind == 8 && !symbol.body.is_empty() {
                    ensure!(
                        symbol.body.len() <= 4096 && self.media_names.len() < 200_000,
                        "KFX symbol table exceeds conversion limits"
                    );
                    let name = std::str::from_utf8(symbol.body)?.to_owned();
                    if let Some(previous) = self.media_names.insert(name, id) {
                        ensure!(previous == id, "conflicting KFX media symbols");
                    }
                }
            }
        }
        let index_size = index_size.unwrap_or(0);
        if index_size == 0 {
            return Ok(());
        }
        // The container index stores 24-byte records. Entity offsets are relative
        // to the end of the container header. Type 417 carries raw media.
        let index = region(
            header_data,
            index_offset.context("missing KFX index offset")?,
            index_size,
        )?;
        ensure!(index.len().is_multiple_of(24), "invalid KFX index size");
        let payload = &component[header_data.len()..];
        for entry in index.chunks_exact(24) {
            let id = little_endian(&entry[..4]);
            let kind = little_endian(&entry[4..8]);
            if metadata_only && !matches!(kind, 258 | 490) {
                continue;
            }
            if !matches!(
                kind,
                145 | 157 | 164 | 258 | 259 | 260 | 266 | 389 | 391 | 393 | 417 | 490 | 538 | 608
            ) {
                continue;
            }
            let entity = region(
                payload,
                little_endian(&entry[8..16]),
                little_endian(&entry[16..24]),
            )?;
            let header = region(entity, 0, 10)?;
            ensure!(&header[..6] == b"ENTY\x01\0", "unsupported KFX entity");
            let offset = usize::try_from(little_endian(&header[6..10]))?;
            ensure!(offset >= 10, "invalid KFX entity header");
            let media = entity.get(offset..).context("invalid KFX entity offset")?;
            if kind == 417 {
                if media.starts_with(b"%PDF-") {
                    ensure!(
                        media.trim_ascii_end().ends_with(b"%%EOF"),
                        "embedded PDF is missing its end marker"
                    );
                    self.pdf_ids.push(id);
                } else if crate::epub::image_type(media).is_none() {
                    continue;
                }
            }
            self.stored_bytes = self
                .stored_bytes
                .checked_add(media.len())
                .context("KFX resource size overflow")?;
            ensure!(
                self.stored_bytes <= 512 * 1024 * 1024 && self.fragments.len() < 100_000,
                "KFX resources exceed conversion limits"
            );
            ensure!(
                self.fragments.insert((kind, id), media.to_vec()).is_none(),
                "duplicate KFX fragment"
            );
        }
        Ok(())
    }

    pub(crate) fn fragment(&self, kind: u64, id: u64) -> Result<ion::Value<'_>> {
        fragment_value(
            self.fragments
                .get(&(kind, id))
                .with_context(|| format!("missing KFX fragment {kind}:{id}"))?,
        )
        .and_then(|value| {
            ensure!(
                value.kind == if kind == 389 { 11 } else { 13 },
                "invalid KFX fragment {kind}:{id}"
            );
            Ok(value)
        })
    }

    pub fn has_pdf(&self) -> bool {
        !self.pdf_ids.is_empty()
    }

    pub(crate) fn sections(&self) -> Result<&[u8]> {
        for kind in [538, 258] {
            for &(_, id) in self.fragments.keys().filter(|(entry, _)| *entry == kind) {
                if let Some(orders) = field(self.fragment(kind, id)?.body, 169)? {
                    let mut orders = list(orders)?;
                    let order = ion::value(&mut orders)?;
                    ensure!(
                        orders.is_empty(),
                        "multiple KFX reading orders are unsupported"
                    );
                    return list(required(order.body, 170)?);
                }
            }
        }
        anyhow::bail!("missing KFX reading order")
    }

    pub(crate) fn media_id(&self, location: ion::Value<'_>) -> Result<u64> {
        match location.kind {
            7 => number(location, 7),
            8 => self
                .media_names
                .get(std::str::from_utf8(location.body)?)
                .copied()
                .context("missing KFX media location"),
            _ => anyhow::bail!("unsupported KFX media location"),
        }
    }

    pub fn write_pdf(mut self, output: &mut impl std::io::Write) -> Result<usize> {
        use lopdf::{Document, Object, dictionary};

        ensure!(
            !self.pdf_ids.is_empty(),
            "no embedded PDF found in this book"
        );
        let mut documents = self.fragments.keys().filter(|(kind, _)| *kind == 538);
        let Some(&(_, document_id)) = documents.next() else {
            ensure!(
                self.pdf_ids.len() == 1,
                "missing KFX reading order for embedded PDFs"
            );
            output.write_all(&self.fragments[&(417, self.pdf_ids[0])])?;
            return Ok(1);
        };
        ensure!(documents.next().is_none(), "multiple KFX document records");
        let pages = self.page_order(document_id)?;
        ensure!(!pages.is_empty(), "no pages in KFX reading order");
        if self.pdf_ids.len() == 1
            && pages.iter().enumerate().all(|(index, &(id, page))| {
                id == self.pdf_ids[0] && usize::try_from(page).ok() == Some(index)
            })
        {
            let bytes = &self.fragments[&(417, self.pdf_ids[0])];
            let document = Document::load_mem(bytes)?;
            if !document.is_encrypted() && document.get_pages().len() == pages.len() {
                output.write_all(bytes)?;
                return Ok(pages.len());
            }
        }
        let mut result = Document::with_version("1.7");
        let root = result.new_object_id();
        let mut imported = std::collections::HashMap::new();
        let mut kids = Vec::new();
        for &(id, page) in &pages {
            if let std::collections::hash_map::Entry::Vacant(entry) = imported.entry(id) {
                let bytes = self
                    .fragments
                    .remove(&(417, id))
                    .context("missing KFX page media")?;
                if bytes.starts_with(b"%PDF-") {
                    entry.insert(crate::pdf_pages::import(&mut result, &bytes)?);
                } else {
                    entry.insert(crate::pdf_pages::jpeg(&mut result, bytes)?);
                }
            }
            let mut page = imported[&id]
                .get(usize::try_from(page)?)
                .context("KFX page index is outside its PDF")?
                .clone();
            page.set("Parent", root);
            kids.push(Object::Reference(result.add_object(page)));
        }
        result.objects.insert(
            root,
            dictionary! {
                "Type" => "Pages", "Kids" => kids, "Count" => i64::try_from(pages.len())?
            }
            .into(),
        );
        let catalog = result.add_object(dictionary! { "Type" => "Catalog", "Pages" => root });
        result.trailer.set("Root", catalog);
        result.prune_objects();
        result.save_to(output)?;
        Ok(pages.len())
    }

    fn page_order(&self, document: u64) -> Result<Vec<(u64, u64)>> {
        let mut orders = list(required(self.fragment(538, document)?.body, 169)?)?;
        let order = ion::value(&mut orders)?;
        ensure!(
            orders.is_empty(),
            "multiple KFX reading orders are unsupported"
        );
        let mut sections = list(required(order.body, 170)?)?;
        let mut pages = Vec::new();
        while !sections.is_empty() {
            let section = self.fragment(260, number(ion::value(&mut sections)?, 7)?)?;
            let mut templates = list(required(section.body, 141)?)?;
            let mut story = None;
            while !templates.is_empty() {
                let template = ion::value(&mut templates)?;
                let id = number(required(template.body, 176)?, 7)?;
                ensure!(
                    story.replace(id).is_none_or(|previous| previous == id),
                    "multiple distinct KFX page templates"
                );
            }
            let story = self.fragment(259, story.context("missing KFX page template")?)?;
            let mut images = Vec::new();
            collect_images(list(required(story.body, 146)?)?, 0, &mut images)?;
            ensure!(
                !images.is_empty(),
                "KFX section has no supported page images"
            );
            for image in images {
                let resource = self.fragment(164, image)?;
                let format = number(required(resource.body, 161)?, 7)?;
                ensure!(
                    matches!(format, 285 | 565),
                    "unsupported print-replica page format {format}"
                );
                let location = required(resource.body, 165)?;
                let id = match location.kind {
                    7 => number(location, 7)?,
                    8 => *self
                        .media_names
                        .get(std::str::from_utf8(location.body)?)
                        .context("missing KFX page location")?,
                    _ => anyhow::bail!("unsupported KFX page location"),
                };
                let page = field(resource.body, 564)?
                    .map(|value| number(value, 2))
                    .transpose()?
                    .unwrap_or(0);
                ensure!(pages.len() < 10_000, "print replica exceeds page limit");
                pages.push((id, page));
            }
        }
        Ok(pages)
    }
}

pub(crate) fn record_value(data: &[u8]) -> Result<ion::Value<'_>> {
    let value = fragment_value(data)?;
    ensure!(value.kind == 13, "invalid KFX record");
    Ok(value)
}

fn fragment_value(data: &[u8]) -> Result<ion::Value<'_>> {
    let mut data = data.strip_prefix(b"\xe0\x01\x00\xea").unwrap_or(data);
    let value = ion::value(&mut data)?;
    ensure!(data.is_empty(), "invalid KFX record");
    Ok(value)
}

pub(crate) fn field(mut record: &[u8], wanted: usize) -> Result<Option<ion::Value<'_>>> {
    while !record.is_empty() {
        let (id, value) = ion::field(&mut record)?;
        if id == wanted {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

pub(crate) fn required(record: &[u8], wanted: usize) -> Result<ion::Value<'_>> {
    field(record, wanted)?.with_context(|| format!("missing KFX field {wanted}"))
}

pub(crate) fn number(value: ion::Value<'_>, kind: u8) -> Result<u64> {
    ensure!(
        value.kind == kind && value.body.len() <= 8,
        "invalid KFX number"
    );
    Ok(value
        .body
        .iter()
        .fold(0, |number, &byte| (number << 8) | u64::from(byte)))
}

pub(crate) fn list(value: ion::Value<'_>) -> Result<&[u8]> {
    ensure!(value.kind == 11, "expected a KFX list");
    Ok(value.body)
}

fn collect_images(mut items: &[u8], depth: usize, images: &mut Vec<u64>) -> Result<()> {
    ensure!(depth < 32, "KFX page layout is too deeply nested");
    while !items.is_empty() {
        let item = ion::value(&mut items)?;
        ensure!(item.kind == 13, "unsupported KFX page layout");
        if field(item.body, 69)?.is_some() {
            continue;
        }
        if let Some(image) = field(item.body, 175)? {
            ensure!(images.len() < 64, "too many images in KFX section");
            images.push(number(image, 7)?);
        }
        for key in [145, 146] {
            if let Some(children) = field(item.body, key)? {
                collect_images(list(children)?, depth + 1, images)?;
            }
        }
    }
    Ok(())
}
