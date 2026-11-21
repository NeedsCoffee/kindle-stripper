use crate::{
    ion::{self, Value},
    kfx_pdf::{Book, field, list, number, record_value, required},
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write as _,
    io::{Seek, Write},
};
use zip::{ZipWriter, write::SimpleFileOptions};

pub fn image_type(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(b"\xff\xd8") {
        Some(("jpg", "image/jpeg"))
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("png", "image/png"))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(("gif", "image/gif"))
    } else {
        None
    }
}

pub(crate) fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\t' | '\n' | '\r' => escaped.push(character),
            c if c < ' ' || matches!(c, '\u{fffe}' | '\u{ffff}') => escaped.push('\u{fffd}'),
            c => escaped.push(c),
        }
    }
    escaped
}

pub(crate) fn string(value: Value<'_>) -> Result<&str> {
    ensure!(value.kind == 8, "expected KFX text");
    Ok(std::str::from_utf8(value.body)?)
}

pub(crate) fn entries<'a>(mut items: &'a [u8]) -> impl Iterator<Item = Result<Value<'a>>> {
    std::iter::from_fn(move || {
        if items.is_empty() {
            return None;
        }
        let value = ion::value(&mut items);
        if value.is_err() {
            items = &[];
        }
        Some(value)
    })
}

#[derive(Default)]
pub(crate) struct Metadata<'a> {
    pub title: Option<&'a str>,
    language: Option<&'a str>,
    pub identifier: Option<&'a str>,
    pub date: Option<&'a str>,
    pub authors: Vec<&'a str>,
    publisher: Option<&'a str>,
    cover: Option<&'a str>,
    fixed_layout: bool,
}

pub(crate) fn metadata(book: &Book) -> Result<Metadata<'_>> {
    let mut result = Metadata::default();
    for (&(kind, _), bytes) in &book.fragments {
        if kind != 490 {
            continue;
        }
        for group in entries(list(required(record_value(bytes)?.body, 491)?)?) {
            let group = group?;
            let category = string(required(group.body, 495)?)?;
            for pair in entries(list(required(group.body, 258)?)?) {
                let pair = pair?;
                let key = string(required(pair.body, 492)?)?;
                if category == "kindle_capability_metadata" && key == "yj_fixed_layout" {
                    result.fixed_layout = true;
                }
                if category != "kindle_title_metadata" {
                    continue;
                }
                let target = match key {
                    "title" => &mut result.title,
                    "language" => &mut result.language,
                    "issue_date" => &mut result.date,
                    "ASIN" => &mut result.identifier,
                    "publisher" => &mut result.publisher,
                    "cover_image" => &mut result.cover,
                    "author" => {
                        result.authors.push(string(required(pair.body, 307)?)?);
                        continue;
                    }
                    _ => continue,
                };
                let value = required(pair.body, 307)?;
                if value.kind == 8 {
                    *target = Some(string(value)?);
                }
            }
        }
    }
    for &(kind, id) in book.fragments.keys() {
        if kind != 258 {
            continue;
        }
        let legacy = book.fragment(kind, id)?;
        if result.authors.is_empty()
            && let Some(value) = field(legacy.body, 222)?.filter(|value| value.kind == 8)
        {
            result.authors.push(string(value)?);
        }
        for (key, target) in [
            (10, &mut result.language),
            (153, &mut result.title),
            (219, &mut result.date),
            (232, &mut result.publisher),
        ] {
            if target.is_none_or(|text| text.trim().is_empty())
                && let Some(value) = field(legacy.body, key)?.filter(|value| value.kind == 8)
            {
                *target = Some(string(value)?);
            }
        }
    }
    Ok(result)
}

impl Metadata<'_> {
    pub fn urn(&self, fallback: &str) -> Option<String> {
        self.identifier
            .filter(|id| crate::is_asin(id))
            .or_else(|| {
                let id = fallback.strip_suffix("_EBOK").unwrap_or(fallback);
                crate::is_asin(id).then_some(id)
            })
            .map(|id| format!("urn:asin:{}", id.to_ascii_uppercase()))
    }

    pub fn cover_media(&self, book: &Book) -> Result<Option<u64>> {
        let explicit = self
            .cover
            .and_then(|name| book.media_names.get(name))
            .copied();
        let mut candidates = Vec::new();
        if let Some(id) = explicit {
            candidates.push(id);
        }
        for &(kind, id) in book.fragments.keys() {
            if kind == 258
                && let Some(cover) = field(book.fragment(kind, id)?.body, 424)?
                && let Ok(id) = book.media_id(cover)
            {
                candidates.push(id);
            }
        }
        for id in candidates {
            let media = if book.fragments.contains_key(&(164, id)) {
                let resource = book.fragment(164, id)?;
                field(resource.body, 165)?.and_then(|value| book.media_id(value).ok())
            } else {
                Some(id)
            };
            if let Some(id) = media
                && book
                    .fragments
                    .get(&(417, id))
                    .is_some_and(|data| image_type(data).is_some())
            {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }
}

pub(crate) fn modified_now() -> String {
    let mut time = windows_sys::Win32::Foundation::SYSTEMTIME::default();
    // GetSystemTime fills the structure with UTC fields.
    unsafe { windows_sys::Win32::System::SystemInformation::GetSystemTime(&mut time) };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute, time.wSecond
    )
}

pub fn is_fixed_layout(book: &Book) -> Result<bool> {
    if book.fragments.keys().any(|&(kind, _)| kind == 490) {
        Ok(metadata(book)?.fixed_layout)
    } else {
        Ok(book.has_pdf())
    }
}

struct Content<'a> {
    book: &'a Book,
    text: HashMap<u64, Vec<&'a str>>,
    positions: crate::epub_navigation::Positions,
    requested_offsets: crate::epub_navigation::Offsets,
    emitted_offsets: crate::epub_navigation::Offsets,
    section: usize,
    images: BTreeMap<u64, (&'static str, &'static str)>,
}

impl<'a> Content<'a> {
    fn new(book: &'a Book) -> Result<Self> {
        let mut text = HashMap::new();
        let mut count = 0;
        for (&(kind, id), bytes) in &book.fragments {
            if kind != 145 {
                continue;
            }
            let record = record_value(bytes)?;
            let mut strings = Vec::new();
            for item in entries(list(required(record.body, 146)?)?) {
                count += 1;
                ensure!(count <= 1_000_000, "too many KFX text entries");
                strings.push(string(item?)?);
            }
            text.insert(id, strings);
        }
        Ok(Self {
            book,
            text,
            positions: HashMap::new(),
            requested_offsets: crate::epub_navigation::requested_offsets(book)?,
            emitted_offsets: HashMap::new(),
            section: 0,
            images: BTreeMap::new(),
        })
    }

    fn resolve_text(&self, value: Value<'a>) -> Result<&'a str> {
        if value.kind == 8 {
            return string(value);
        }
        ensure!(value.kind == 13, "unsupported KFX text representation");
        let id = number(required(value.body, 4)?, 7)?;
        let index = usize::try_from(number(required(value.body, 403)?, 2)?)?;
        self.text
            .get(&id)
            .and_then(|strings| strings.get(index))
            .copied()
            .context("missing KFX text entry")
    }

    fn image(&mut self, id: u64) -> Result<(u64, &'static str)> {
        let resource = self.book.fragment(164, id)?;
        let media = self.book.media_id(required(resource.body, 165)?)?;
        let bytes = self
            .book
            .fragments
            .get(&(417, media))
            .context("missing or unsupported KFX image")?;
        let (extension, mime) = image_type(bytes).context("unsupported EPUB image type")?;
        self.images.insert(media, (extension, mime));
        Ok((media, extension))
    }

    fn record_position(&mut self, fields: &[u8], body: &mut String) -> Result<Option<u64>> {
        if let Some(id) = crate::epub_navigation::position(fields)?
            && let std::collections::hash_map::Entry::Vacant(position) = self.positions.entry(id)
        {
            position.insert(self.section);
            write!(body, "<span id=\"kfx-{id}\"></span>")?;
            return Ok(Some(id));
        }
        Ok(None)
    }

    fn render(
        &mut self,
        mut node: Value<'a>,
        depth: usize,
        body: &mut String,
        heading: &mut Option<String>,
    ) -> Result<()> {
        ensure!(
            depth < 48 && body.len() < 32 * 1024 * 1024,
            "KFX section exceeds EPUB conversion limits"
        );
        if node.kind == 7 {
            node = self.book.fragment(608, number(node, 7)?)?;
        }
        if node.kind == 8 {
            let text = string(node)?;
            ensure!(
                text.len() <= 2 * 1024 * 1024,
                "KFX text exceeds EPUB conversion limits"
            );
            write!(body, "<span>{}</span>", escape(text))?;
            return Ok(());
        }
        ensure!(
            node.kind == 13,
            "unsupported KFX content node of Ion type {}",
            node.kind
        );
        let fields = node.body;
        let position = self.record_position(fields, body)?;
        if let Some(text) = field(fields, 145)? {
            let text = self.resolve_text(text)?;
            ensure!(
                text.len() <= 2 * 1024 * 1024,
                "KFX text exceeds EPUB conversion limits"
            );
            if heading.is_none() && !text.trim().is_empty() {
                *heading = Some(
                    text.split_whitespace()
                        .take(10)
                        .collect::<Vec<_>>()
                        .join(" ")
                        .chars()
                        .take(80)
                        .collect(),
                );
            }
            let mut anchors = Vec::new();
            if let Some(id) = position
                && let Some(offsets) = self.requested_offsets.get(&id)
            {
                let limit = text.chars().count();
                anchors.extend(offsets.range(..=limit).map(|&offset| (offset, id)));
                self.emitted_offsets
                    .entry(id)
                    .or_default()
                    .extend(anchors.iter().map(|&(offset, _)| offset));
            }
            crate::epub_style::paragraph(self.book, fields, text, &anchors, body)?;
        }
        if let Some(image) = field(fields, 175)?
            && field(fields, 159)?
                .map(|value| number(value, 7))
                .transpose()?
                == Some(271)
        {
            let (id, extension) = self.image(number(image, 7)?)?;
            let alt = field(fields, 584)?.map(string).transpose()?.unwrap_or("");
            let style = crate::epub_style::image_style(self.book, fields)?;
            let inline = field(fields, 601)?
                .map(|value| number(value, 7))
                .transpose()?
                == Some(283);
            let link = crate::epub_navigation::link_attribute(self.book, fields)?;
            if !inline {
                let alignment = crate::epub_style::alignment_style(self.book, fields)?;
                write!(body, "<p{alignment}>")?;
            }
            if let Some(link) = &link {
                write!(body, "<a {link}>")?;
            }
            write!(
                body,
                "<img src=\"media-{id}.{extension}\" alt=\"{}\"{style}/>",
                escape(alt)
            )?;
            if link.is_some() {
                body.push_str("</a>");
            }
            if !inline {
                body.push_str("</p>\n");
            }
        }
        let children = field(fields, 146)?.map(list).transpose()?;
        let story = field(fields, 176)?
            .map(|value| self.book.fragment(259, number(value, 7)?))
            .transpose()?;
        if children.is_some() || story.is_some() {
            let mixed = children.is_some_and(|items| {
                entries(items).any(|child| child.is_ok_and(|value| value.kind == 8))
            });
            let style = if mixed {
                crate::epub_style::block_style(self.book, fields)?
            } else {
                crate::epub_style::alignment_style(self.book, fields)?
            };
            let wrap = mixed || !style.is_empty();
            if wrap {
                write!(body, "<div{style}>")?;
            }
            if let Some(children) = children {
                self.render_list(children, depth + 1, body, heading)?;
            }
            if let Some(story) = story {
                self.record_position(story.body, body)?;
                self.render_list(list(required(story.body, 146)?)?, depth + 1, body, heading)?;
            }
            if wrap {
                body.push_str("</div>\n");
            }
        }
        Ok(())
    }

    fn render_list(
        &mut self,
        items: &'a [u8],
        depth: usize,
        body: &mut String,
        heading: &mut Option<String>,
    ) -> Result<()> {
        for node in entries(items) {
            self.render(node?, depth, body, heading)?;
        }
        Ok(())
    }
}

fn add_file(zip: &mut ZipWriter<impl Write + Seek>, name: &str, bytes: &[u8]) -> Result<()> {
    zip.start_file(name, SimpleFileOptions::default())?;
    zip.write_all(bytes)?;
    Ok(())
}

fn xhtml(title: &str, language: &str, body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" xml:lang="{language}" lang="{language}">
<head><title>{title}</title><style>img {{ max-width: 100%; height: auto; }}</style></head>
<body>{body}</body></html>"#
    )
}

pub fn write(book: &Book, output: impl Write + Seek, fallback: &str) -> Result<()> {
    let metadata = metadata(book)?;
    let title = escape(metadata.title.unwrap_or(fallback));
    let language = escape(metadata.language.unwrap_or("und"));
    let identifier = escape(
        &metadata
            .urn(fallback)
            .unwrap_or_else(|| fallback.to_owned()),
    );
    let mut content = Content::new(book)?;
    let mut zip = ZipWriter::new(output);
    add_file(&mut zip, "mimetype", b"application/epub+zip")?;
    add_file(&mut zip, "META-INF/container.xml", br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#)?;
    let mut manifest = String::from(
        r#"<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>"#,
    );
    let mut spine = String::new();
    let mut navigation = String::new();
    let mut total_bytes = 0usize;
    let mut chapters = Vec::new();
    for (index, section) in entries(book.sections()?).enumerate() {
        ensure!(index < 10_000, "too many KFX sections");
        let section = book.fragment(260, number(section?, 7)?)?;
        content.section = index + 1;
        let mut body = String::new();
        let mut heading = None;
        content.render_list(
            list(required(section.body, 141)?)?,
            0,
            &mut body,
            &mut heading,
        )?;
        if body.is_empty() {
            continue;
        }
        total_bytes += body.len();
        ensure!(
            total_bytes <= 256 * 1024 * 1024,
            "EPUB chapter output exceeds size limit"
        );
        let name = format!("section-{}.xhtml", index + 1);
        let label = escape(&heading.unwrap_or_else(|| format!("Section {}", index + 1)));
        chapters.push((name.clone(), label.clone(), body));
        writeln!(
            manifest,
            r#"<item id="s{index}" href="{name}" media-type="application/xhtml+xml"/>"#
        )?;
        writeln!(spine, r#"<itemref idref="s{index}"/>"#)?;
        writeln!(navigation, r#"<li><a href="{name}">{label}</a></li>"#)?;
    }
    ensure!(!spine.is_empty(), "no supported reflowable content found");
    for (name, label, body) in chapters {
        let body = crate::epub_navigation::resolve_links(
            &body,
            &content.positions,
            &content.emitted_offsets,
        );
        add_file(
            &mut zip,
            &format!("EPUB/{name}"),
            xhtml(&label, &language, &body).as_bytes(),
        )?;
    }
    if let Some(original) =
        crate::epub_navigation::contents(book, &content.positions, &content.emitted_offsets)?
    {
        navigation = original;
    }
    let cover = metadata.cover_media(book)?;
    if let Some(id) = cover
        && let Some((ext, mime)) = image_type(&book.fragments[&(417, id)])
    {
        content.images.insert(id, (ext, mime));
    }
    for (id, (extension, mime)) in content.images {
        let name = format!("media-{id}.{extension}");
        add_file(
            &mut zip,
            &format!("EPUB/{name}"),
            &book.fragments[&(417, id)],
        )?;
        let properties = if Some(id) == cover {
            r#" properties="cover-image""#
        } else {
            ""
        };
        writeln!(
            manifest,
            r#"<item id="m{id}" href="{name}" media-type="{mime}"{properties}/>"#
        )?;
    }
    let nav = xhtml(
        "Contents",
        &language,
        &format!(r#"<nav epub:type="toc" id="toc"><h1>Contents</h1><ol>{navigation}</ol></nav>"#),
    );
    add_file(&mut zip, "EPUB/nav.xhtml", nav.as_bytes())?;
    let mut authors = String::new();
    for author in metadata.authors {
        writeln!(authors, "<dc:creator>{}</dc:creator>", escape(author))?;
    }
    let publisher = metadata
        .publisher
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("<dc:publisher>{}</dc:publisher>", escape(value)))
        .unwrap_or_default();
    let date = metadata
        .date
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("<dc:date>{}</dc:date>", escape(value)))
        .unwrap_or_default();
    let modified = modified_now();
    let package = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="book-id" xml:lang="{language}">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:identifier id="book-id">{identifier}</dc:identifier><dc:title>{title}</dc:title><dc:language>{language}</dc:language>{authors}{publisher}{date}
<meta property="dcterms:modified">{modified}</meta></metadata>
<manifest>{manifest}</manifest><spine>{spine}</spine></package>"#
    );
    add_file(&mut zip, "EPUB/package.opf", package.as_bytes())?;
    zip.finish()?;
    Ok(())
}
