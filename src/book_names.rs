use crate::{ContentKeys, epub, is_asin, kfx_pdf::Book, kindle_store, voucher, windows_paths};
use std::{
    ffi::OsStr,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

pub struct BookName {
    title: String,
    author: Option<String>,
    asin: Option<String>,
}

pub fn from_book(book: &Book) -> Option<BookName> {
    let metadata = epub::metadata(book).ok()?;
    let title = display_text(metadata.title?);
    if title.is_empty() {
        return None;
    }
    Some(BookName {
        title,
        author: metadata
            .authors
            .first()
            .map(|value| display_text(value))
            .filter(|value| !value.is_empty()),
        asin: metadata
            .identifier
            .filter(|id| is_asin(id))
            .map(str::to_ascii_uppercase),
    })
}

fn display_text(value: &str) -> String {
    value
        .split(|ch: char| ch.is_whitespace() || ch.is_control())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn metadata_first(components: &[PathBuf]) -> impl Iterator<Item = &PathBuf> {
    components
        .iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        })
        .chain(components.iter().filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("azw"))
        }))
}

fn read_name(path: &Path, keys: &ContentKeys) -> Option<BookName> {
    let bytes = crate::read_component(path, keys).ok()?;
    let mut book = Book::default();
    book.add_metadata_component(&bytes).ok()?;
    from_book(&book)
}

pub fn read(components: &[PathBuf], keys: &ContentKeys) -> Option<BookName> {
    metadata_first(components).find_map(|path| read_name(path, keys))
}

pub fn list(books: &[(String, PathBuf)], numbered: bool) {
    let mut credentials = None;
    let empty_keys = ContentKeys::new();
    for (index, (asin, folder)) in books.iter().enumerate() {
        let name = crate::collect_components(folder)
            .ok()
            .and_then(|(_, components)| {
                let mut keys = None;
                metadata_first(&components).find_map(|path| {
                    let mut header = [0; 4];
                    File::open(path).ok()?.read_exact(&mut header).ok()?;
                    if &header == b"CONT" {
                        return read_name(path, &empty_keys);
                    }
                    if !crate::drmion::MAGIC.starts_with(&header) {
                        return None;
                    }
                    let credentials = credentials
                        .get_or_insert_with(kindle_store::credential_database)
                        .as_ref()
                        .ok()?;
                    let keys = keys
                        .get_or_insert_with(|| voucher::derive_keys(folder, credentials, false))
                        .as_ref()
                        .ok()?;
                    read_name(path, keys)
                })
            });
        let number = numbered
            .then(|| format!("{}  ", index + 1))
            .unwrap_or_default();
        match name {
            Some(name) => println!("{number}{asin}  {}", name.title),
            None => println!("{number}{asin}"),
        }
    }
}

fn filename_part(value: &str, limit: usize) -> String {
    let filtered: String = value
        .chars()
        .map(|ch| {
            if ch.is_control() || "<>:\"/\\|?*".contains(ch) {
                ' '
            } else {
                ch
            }
        })
        .collect();
    let filtered = display_text(&filtered);
    let mut units = 0;
    let truncated = filtered
        .chars()
        .take_while(|ch| {
            units += ch.len_utf16();
            units <= limit
        })
        .collect::<String>();
    let boundary = if truncated.len() < filtered.len()
        && !filtered[truncated.len()..].starts_with(' ')
        && let Some((before, last)) = truncated.rsplit_once(' ')
        && last.chars().count() <= 16
    {
        before
    } else {
        &truncated
    };
    boundary.trim_matches([' ', '.']).to_owned()
}

pub fn filename(
    metadata: Option<BookName>,
    asin: Option<&str>,
    original: &OsStr,
    extension: &str,
) -> PathBuf {
    let original = original.to_string_lossy();
    let source_asin = original.strip_suffix("_EBOK").unwrap_or(&original);
    let asin = asin
        .or_else(|| metadata.as_ref().and_then(|name| name.asin.as_deref()))
        .or_else(|| is_asin(source_asin).then_some(source_asin));
    let suffix = asin.map_or_else(String::new, |asin| format!(" [{asin}]"));
    let author = metadata
        .as_ref()
        .and_then(|name| name.author.as_deref())
        .map(|author| filename_part(author, 48))
        .filter(|author| !author.is_empty())
        .map_or_else(String::new, |author| format!(" - {author}"));
    // Bound the full filename in UTF-16 units, leaving space for destination directories.
    let budget =
        160 - 2 - "kfx-zip".len() - suffix.encode_utf16().count() - author.encode_utf16().count();
    let title = metadata
        .as_ref()
        .map(|name| filename_part(&name.title, budget))
        .filter(|title| !title.is_empty());
    let mut name = match title {
        Some(title) => format!("{title}{author}{suffix}.{extension}"),
        None => {
            let fallback = filename_part(asin.unwrap_or(&original), 140);
            let fallback = if fallback.is_empty() {
                "Kindle book"
            } else {
                &fallback
            };
            format!("{fallback}.{extension}")
        }
    };
    if windows_paths::validate_component(&name).is_err() {
        name.insert(0, '_');
    }
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_windows_names_and_retains_the_asin_and_extension() {
        let original = OsStr::new("B008E8XV56_EBOK");
        let name = filename(
            Some(BookName {
                title: format!("CON. Intro: {}?", "📖".repeat(160)),
                author: Some("A/B".to_owned()),
                asin: None,
            }),
            None,
            original,
            "kfx-zip",
        );
        let name = name.to_str().unwrap();
        assert!(name.starts_with("_CON. Intro "));
        assert!(name.ends_with(" - A B [B008E8XV56].kfx-zip"));
        assert!(name.encode_utf16().count() <= 160);
        windows_paths::validate_component(name).unwrap();
    }
}
