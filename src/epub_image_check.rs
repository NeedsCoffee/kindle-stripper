use anyhow::{Context, Result, ensure};
use quick_xml::{
    Reader,
    events::{BytesStart, Event},
};
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Seek, SeekFrom},
};
use zip::ZipArchive;

const MAX_XML: u64 = 64 * 1024 * 1024;

pub fn check_boko_images(file: &mut (impl Read + Seek)) -> Result<()> {
    file.seek(SeekFrom::Start(0))?;
    let mut archive = ZipArchive::new(file)?;
    let (images, mut documents) = manifest(&mut archive)?;
    let mut svg_documents = Vec::new();
    for (path, media) in &images {
        check_image(&mut archive, path, media)
            .with_context(|| format!("checking packaged image {path}"))?;
        if media == "image/svg+xml" {
            svg_documents.push(path.clone());
        }
    }
    documents.extend(svg_documents);
    let mut checked = HashSet::new();
    for document in documents {
        if !checked.insert(document.clone()) {
            continue;
        }
        elements(&read_xml(&mut archive, &document)?, |tag| {
            let href = match tag.local_name().as_ref() {
                "img" => attribute(tag, "src")?,
                "image" => attribute(tag, "href")?.or(attribute(tag, "xlink:href")?),
                _ => return Ok(()),
            };
            let href = href
                .filter(|href| !href.is_empty())
                .context("image has no source")?;
            if let Some(path) = resolve(&document, &href)? {
                ensure!(
                    images.contains_key(&path),
                    "{document} references {href}, which is not a packaged EPUB image"
                );
            }
            Ok(())
        })
        .with_context(|| format!("checking image references in {document}"))?;
    }
    Ok(())
}

fn manifest(
    archive: &mut ZipArchive<impl Read + Seek>,
) -> Result<(HashMap<String, String>, Vec<String>)> {
    // This is the publication path emitted by boko's EPUB writer.
    let package = "OEBPS/content.opf";
    let mut images = HashMap::new();
    let mut documents = Vec::new();
    elements(&read_xml(archive, package)?, |tag| {
        if tag.local_name().as_ref() != "item" {
            return Ok(());
        }
        let Some(media) = attribute(tag, "media-type")? else {
            return Ok(());
        };
        if !media.starts_with("image/") && media != "application/xhtml+xml" {
            return Ok(());
        }
        let href = attribute(tag, "href")?.context("EPUB manifest item has no href")?;
        if let Some(path) = resolve(package, &href)? {
            if media.starts_with("image/") {
                ensure!(
                    images.insert(path, media).is_none(),
                    "duplicate EPUB image manifest entry"
                );
            } else {
                documents.push(path);
            }
        }
        Ok(())
    })?;

    Ok((images, documents))
}

pub fn repair_boko_images(
    file: &mut std::fs::File,
    source: &crate::kfx_pdf::Book,
) -> Result<usize> {
    file.seek(SeekFrom::Start(0))?;
    let mut archive = ZipArchive::new(&mut *file)?;
    let (images, _) = manifest(&mut archive)?;
    let mut replacements = std::collections::BTreeMap::new();
    for (path, media) in &images {
        if check_image(&mut archive, path, media).is_ok() {
            continue;
        }
        let Some(name) = path.strip_prefix("OEBPS/") else {
            continue;
        };
        if let Some(bytes) = source_image(source, name, media)? {
            replacements.insert(path.as_str(), bytes);
        }
    }
    let count = replacements.len();
    if count == 0 {
        return Ok(0);
    }

    drop(archive);
    rewrite_entries(file, replacements)?;
    Ok(count)
}

pub(crate) fn rewrite_entries(
    file: &mut std::fs::File,
    mut replacements: std::collections::BTreeMap<&str, &[u8]>,
) -> Result<()> {
    use std::io::Write;
    file.seek(SeekFrom::Start(0))?;
    let mut archive = ZipArchive::new(&mut *file)?;
    let mut repaired = tempfile::tempfile()?;
    let mut writer = zip::ZipWriter::new(&mut repaired);
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index)?;
        if let Some(bytes) = replacements.remove(entry.name()) {
            writer.start_file(entry.name(), zip::write::SimpleFileOptions::default())?;
            writer.write_all(bytes)?;
        } else {
            writer.raw_copy_file(entry)?;
        }
    }
    for (path, bytes) in replacements {
        writer.start_file(path, zip::write::SimpleFileOptions::default())?;
        writer.write_all(bytes)?;
    }
    writer.finish()?;
    drop(archive);
    repaired.seek(SeekFrom::Start(0))?;
    file.seek(SeekFrom::Start(0))?;
    file.set_len(0)?;
    std::io::copy(&mut repaired, file)?;
    Ok(())
}

fn source_image<'a>(
    source: &'a crate::kfx_pdf::Book,
    name: &str,
    media: &str,
) -> Result<Option<&'a [u8]>> {
    use crate::kfx_pdf::field;
    let mut ids = HashSet::new();
    if let Some(&id) = source.media_names.get(name) {
        if source.fragments.contains_key(&(417, id)) {
            ids.insert(id);
        }
        if source.fragments.contains_key(&(164, id)) {
            let resource = source.fragment(164, id)?;
            if let Some(location) = field(resource.body, 165)?
                && let Ok(id) = source.media_id(location)
            {
                ids.insert(id);
            }
        }
    }
    if ids.len() != 1 {
        return Ok(None);
    }
    Ok(ids
        .into_iter()
        .next()
        .and_then(|id| source.fragments.get(&(417, id)))
        .filter(|bytes| crate::epub::image_type(bytes).is_some_and(|(_, mime)| mime == media))
        .map(Vec::as_slice))
}

pub(crate) fn read_xml(archive: &mut ZipArchive<impl Read + Seek>, path: &str) -> Result<Vec<u8>> {
    let entry = archive
        .by_name(path)
        .with_context(|| format!("missing EPUB entry {path}"))?;
    ensure!(
        entry.size() <= MAX_XML,
        "EPUB XML exceeds size limit: {path}"
    );
    let mut data = Vec::new();
    entry.take(MAX_XML + 1).read_to_end(&mut data)?;
    ensure!(
        data.len() as u64 <= MAX_XML,
        "EPUB XML exceeds size limit: {path}"
    );
    Ok(data)
}

fn elements(xml: &[u8], mut visit: impl FnMut(&BytesStart<'_>) -> Result<()>) -> Result<()> {
    let mut reader = Reader::from_reader(xml);
    loop {
        match reader.read_event()? {
            Event::Start(tag) | Event::Empty(tag) => visit(&tag)?,
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

pub(crate) fn attribute(tag: &BytesStart<'_>, key: &str) -> Result<Option<String>> {
    tag.try_get_attribute(key)?
        .map(|attr| -> Result<_> {
            Ok(attr
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)?
                .into_owned())
        })
        .transpose()
}

pub(crate) fn resolve(document: &str, href: &str) -> Result<Option<String>> {
    let href = href.trim().split(['#', '?']).next().unwrap_or_default();
    if href.is_empty()
        || href.starts_with("//")
        || href
            .split('/')
            .next()
            .is_some_and(|segment| segment.contains(':'))
    {
        return Ok(None);
    }
    let decoded = percent_encoding::percent_decode_str(href).decode_utf8()?;
    ensure!(!decoded.contains(['\\', '\0']), "invalid EPUB image path");
    let mut parts: Vec<_> = if decoded.starts_with('/') {
        Vec::new()
    } else {
        document
            .rsplit_once('/')
            .map(|(dir, _)| dir.split('/').collect())
            .unwrap_or_default()
    };
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                ensure!(
                    parts.pop().is_some(),
                    "EPUB reference escapes the archive root"
                );
            }
            _ => parts.push(part),
        }
    }
    Ok(Some(parts.join("/")))
}

fn check_image(archive: &mut ZipArchive<impl Read + Seek>, path: &str, media: &str) -> Result<()> {
    if media == "image/svg+xml" {
        let mut root = false;
        elements(&read_xml(archive, path)?, |tag| {
            if !root {
                ensure!(
                    tag.local_name().as_ref() == "svg",
                    "expected an SVG root element"
                );
                root = true;
            }
            Ok(())
        })?;
        ensure!(root, "empty SVG image");
        return Ok(());
    }
    let mut header = Vec::new();
    archive.by_name(path)?.take(32).read_to_end(&mut header)?;
    let valid = match media {
        "image/jpeg" => header.starts_with(b"\xff\xd8\xff"),
        "image/png" => header.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/gif" => header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a"),
        "image/webp" => header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WEBP"),
        "image/bmp" => header.starts_with(b"BM"),
        "image/tiff" => header.starts_with(b"II*\0") || header.starts_with(b"MM\0*"),
        "image/avif" => {
            header.get(4..8) == Some(b"ftyp")
                && matches!(header.get(8..12), Some(b"avif" | b"avis"))
        }
        _ => anyhow::bail!("unsupported EPUB image type {media}"),
    };
    ensure!(valid, "image data does not match declared type {media}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::{ZipWriter, write::SimpleFileOptions};

    fn publication(href: &str, image: Option<&[u8]>) -> Result<Cursor<Vec<u8>>> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("OEBPS/content.opf", options)?;
        zip.write_all(br#"<package><manifest><item media-type="image/jpeg" href="picture%20one"/><item media-type="application/xhtml+xml" href="text/chapter.xhtml"/></manifest></package>"#)?;
        zip.start_file("OEBPS/text/chapter.xhtml", options)?;
        write!(zip, r#"<html><body><img src="{href}"/></body></html>"#)?;
        if let Some(image) = image {
            zip.start_file("OEBPS/picture one", options)?;
            zip.write_all(image)?;
        }
        Ok(zip.finish()?)
    }

    #[test]
    fn restores_an_exact_match_and_leaves_unknown_images_for_validation() -> Result<()> {
        let jpeg = b"\xff\xd8\xff\xe0";
        let mut source = crate::kfx_pdf::Book::default();
        source.media_names.insert("picture one".to_owned(), 42);
        source.fragments.insert((417, 42), jpeg.to_vec());
        for image in [None, Some(b"placeholder".as_slice())] {
            let mut file = tempfile::tempfile()?;
            let mut broken = publication("../picture%20one", image)?;
            broken.set_position(0);
            std::io::copy(&mut broken, &mut file)?;
            assert!(check_boko_images(&mut file).is_err());
            assert_eq!(repair_boko_images(&mut file, &source)?, 1);
            check_boko_images(&mut file)?;
            assert_eq!(repair_boko_images(&mut file, &source)?, 0);
        }
        Ok(())
    }

    #[test]
    fn checks_packaged_images_and_relative_references() -> Result<()> {
        let jpeg = b"\xff\xd8\xff\xe0";
        check_boko_images(&mut publication("../picture%20one", Some(jpeg))?)?;
        assert!(check_boko_images(&mut publication("../picture%20one", None)?).is_err());
        assert!(
            check_boko_images(&mut publication("../picture%20one", Some(b"not an image"))?)
                .is_err()
        );
        assert!(check_boko_images(&mut publication("../missing.jpg", Some(jpeg))?).is_err());
        Ok(())
    }
}
