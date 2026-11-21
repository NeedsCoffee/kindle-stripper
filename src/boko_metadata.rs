use crate::{
    epub,
    epub_image_check::{attribute, read_xml, resolve, rewrite_entries},
    kfx_pdf::Book,
};
use anyhow::{Result, ensure};
use quick_xml::{
    Reader, Writer,
    events::{BytesStart, BytesText, Event},
};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
};
use zip::ZipArchive;

const PACKAGE: &str = "OEBPS/content.opf";
const NCX: &str = "OEBPS/toc.ncx";

pub fn apply(file: &mut File, source: &Book, fallback: &str) -> Result<()> {
    let metadata = epub::metadata(source)?;
    let identifier = metadata.urn(fallback);
    let cover = metadata
        .cover_media(source)?
        .map(|id| source.fragments[&(417, id)].as_slice());
    file.seek(SeekFrom::Start(0))?;
    let mut archive = ZipArchive::new(&mut *file)?;
    let input = read_xml(&mut archive, PACKAGE)?;
    let cover_href = cover
        .map(|bytes| find_cover(&mut archive, &input, bytes))
        .transpose()?
        .flatten();
    let package = update_package(
        &input,
        identifier.as_deref(),
        metadata.date,
        cover_href.as_deref(),
    )?;
    let ncx = if let Some(identifier) = identifier.as_deref() {
        Some(update_ncx(&read_xml(&mut archive, NCX)?, identifier)?)
    } else {
        None
    };
    drop(archive);
    let mut replacements = BTreeMap::from([(PACKAGE, package.as_slice())]);
    if let Some(ncx) = &ncx {
        replacements.insert(NCX, ncx);
    }
    rewrite_entries(file, replacements)
}

fn find_cover(
    archive: &mut ZipArchive<&mut File>,
    package: &[u8],
    cover: &[u8],
) -> Result<Option<String>> {
    let mut reader = Reader::from_reader(package);
    let mut matching = None;
    loop {
        match reader.read_event()? {
            Event::Start(tag) | Event::Empty(tag) if tag.local_name().as_ref() == "item" => {
                if !attribute(&tag, "media-type")?.is_some_and(|media| media.starts_with("image/"))
                {
                    continue;
                }
                let Some(href) = attribute(&tag, "href")? else {
                    continue;
                };
                let Some(path) = resolve(PACKAGE, &href)? else {
                    continue;
                };
                let Ok(entry) = archive.by_name(&path) else {
                    continue;
                };
                if entry.size() != cover.len() as u64 {
                    continue;
                }
                let mut bytes = Vec::new();
                entry.take(cover.len() as u64 + 1).read_to_end(&mut bytes)?;
                if bytes == cover {
                    if attribute(&tag, "properties")?.is_some_and(|properties| {
                        properties.split_whitespace().any(|p| p == "cover-image")
                    }) {
                        return Ok(Some(href));
                    }
                    matching.get_or_insert(href);
                }
            }
            Event::Eof => return Ok(matching),
            _ => {}
        }
    }
}

fn set_attribute(tag: &BytesStart<'_>, key: &str, value: &str) -> Result<BytesStart<'static>> {
    let mut updated = tag.clone().into_owned();
    updated.clear_attributes();
    for attr in tag.attributes() {
        let attr = attr?;
        if attr.key.as_ref() != key {
            updated.push_attribute(attr);
        }
    }
    if !value.is_empty() {
        updated.push_attribute((key, value));
    }
    Ok(updated)
}

fn update_package(
    input: &[u8],
    identifier: Option<&str>,
    date: Option<&str>,
    cover: Option<&str>,
) -> Result<Vec<u8>> {
    let mut reader = Reader::from_reader(input);
    let mut writer = Writer::new(Vec::new());
    let mut book_id = String::new();
    let mut wrote_date = false;
    let modified = epub::modified_now();
    loop {
        let event = reader.read_event()?;
        match &event {
            Event::Start(tag) if tag.local_name().as_ref() == "package" => {
                book_id = attribute(tag, "unique-identifier")?.unwrap_or_default();
            }
            Event::Start(tag) if tag.name().as_ref() == "dc:identifier" && !book_id.is_empty() => {
                if let Some(identifier) = identifier
                    && attribute(tag, "id")?.as_deref() == Some(&book_id)
                {
                    replace_text(&mut reader, &mut writer, tag, identifier)?;
                    continue;
                }
            }
            Event::Start(tag) if tag.name().as_ref() == "dc:date" => {
                if let Some(date) = date {
                    replace_text(&mut reader, &mut writer, tag, date)?;
                    wrote_date = true;
                    continue;
                }
            }
            Event::Start(tag)
                if tag.local_name().as_ref() == "meta"
                    && attribute(tag, "property")?.as_deref() == Some("dcterms:modified") =>
            {
                replace_text(&mut reader, &mut writer, tag, &modified)?;
                continue;
            }
            Event::Empty(tag) if tag.local_name().as_ref() == "item" => {
                if let Some(cover) = cover {
                    let properties = attribute(tag, "properties")?.unwrap_or_default();
                    let mut values: Vec<_> = properties
                        .split_whitespace()
                        .filter(|&p| p != "cover-image")
                        .collect();
                    if attribute(tag, "href")?.as_deref() == Some(cover) {
                        values.push("cover-image");
                    }
                    writer.write_event(Event::Empty(set_attribute(
                        tag,
                        "properties",
                        &values.join(" "),
                    )?))?;
                    continue;
                }
            }
            Event::End(tag) if tag.local_name().as_ref() == "metadata" => {
                if let Some(date) = date
                    && !wrote_date
                {
                    writer
                        .create_element("dc:date")
                        .write_text_content(BytesText::new(date))?;
                }
            }
            Event::Eof => return Ok(writer.into_inner()),
            _ => {}
        }
        writer.write_event(event)?;
    }
}

fn replace_text(
    reader: &mut Reader<&[u8]>,
    writer: &mut Writer<Vec<u8>>,
    start: &BytesStart<'_>,
    text: &str,
) -> Result<()> {
    writer.write_event(Event::Start(start.borrow()))?;
    writer.write_event(Event::Text(BytesText::new(text)))?;
    loop {
        match reader.read_event()? {
            Event::End(end) => {
                ensure!(
                    end.name() == start.name(),
                    "unexpected EPUB metadata end tag"
                );
                writer.write_event(Event::End(end))?;
                return Ok(());
            }
            Event::Text(_) | Event::CData(_) | Event::GeneralRef(_) => {}
            _ => anyhow::bail!("unexpected nested EPUB metadata"),
        }
    }
}

fn update_ncx(input: &[u8], identifier: &str) -> Result<Vec<u8>> {
    let mut reader = Reader::from_reader(input);
    let mut writer = Writer::new(Vec::new());
    loop {
        match reader.read_event()? {
            Event::Empty(tag)
                if tag.local_name().as_ref() == "meta"
                    && attribute(&tag, "name")?.as_deref() == Some("dtb:uid") =>
            {
                writer.write_event(Event::Empty(set_attribute(&tag, "content", identifier)?))?;
            }
            Event::Eof => return Ok(writer.into_inner()),
            event => writer.write_event(event)?,
        }
    }
}
