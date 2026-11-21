use crate::{
    epub::{entries, escape, string},
    ion::Value,
    kfx_pdf::{Book, field, list, number},
};
use anyhow::{Result, ensure};
use std::{
    collections::{BTreeSet, HashMap},
    fmt::Write,
};

pub type Positions = HashMap<u64, usize>;

pub fn position(fields: &[u8]) -> Result<Option<u64>> {
    for key in [155, 598] {
        if let Some(value) = field(fields, key)? {
            return match value.kind {
                2 | 7 => Ok(Some(number(Value { kind: 2, ..value }, 2)?)),
                _ => Ok(None),
            };
        }
    }
    Ok(None)
}

fn offset(fields: &[u8]) -> Result<usize> {
    field(fields, 143)?
        .map(|value| Ok(usize::try_from(number(value, 2)?)?))
        .transpose()
        .map(|value| value.unwrap_or(0))
}

pub type Offsets = HashMap<u64, BTreeSet<usize>>;

pub fn requested_offsets(book: &Book) -> Result<Offsets> {
    let mut offsets = Offsets::new();
    for &(kind, id) in book.fragments.keys() {
        match kind {
            266 => {
                if let Some(target) = field(book.fragment(kind, id)?.body, 183)? {
                    collect_offset(target.body, &mut offsets)?;
                }
            }
            389 | 391 | 393 => {
                collect_navigation_offsets(book.fragment(kind, id)?, 0, &mut offsets)?
            }
            _ => {}
        }
    }
    Ok(offsets)
}

fn collect_offset(fields: &[u8], offsets: &mut Offsets) -> Result<()> {
    if let Some(id) = position(fields)? {
        let offset = offset(fields)?;
        if offset != 0 {
            let entry = offsets.entry(id).or_default();
            ensure!(
                entry.len() < 4096,
                "too many KFX targets within one element"
            );
            entry.insert(offset);
        }
    }
    Ok(())
}

fn collect_navigation_offsets(value: Value<'_>, depth: usize, offsets: &mut Offsets) -> Result<()> {
    ensure!(depth < 48, "KFX navigation exceeds nesting limit");
    match value.kind {
        11 => {
            for item in entries(value.body) {
                collect_navigation_offsets(item?, depth + 1, offsets)?;
            }
        }
        13 => {
            if let Some(target) = field(value.body, 246)? {
                collect_offset(target.body, offsets)?;
            }
            for key in [247, 392] {
                if let Some(children) = field(value.body, key)? {
                    collect_navigation_offsets(children, depth + 1, offsets)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn link_attribute(book: &Book, fields: &[u8]) -> Result<Option<String>> {
    let Some(link) = field(fields, 179)? else {
        return Ok(None);
    };
    let anchor = match link.kind {
        7 => Some(number(link, 7)?),
        8 => book.media_names.get(string(link)?).copied(),
        _ => None,
    };
    let Some(anchor) = anchor.filter(|id| book.fragments.contains_key(&(266, *id))) else {
        return Ok(None);
    };
    let anchor = book.fragment(266, anchor)?;
    if let Some(target) = field(anchor.body, 183)?
        && let Some(position) = position(target.body)?
    {
        return Ok(Some(format!(
            "data-kfx-target=\"{position}:{}\"",
            offset(target.body)?
        )));
    }
    if let Some(uri) = field(anchor.body, 186)? {
        let uri = string(uri)?;
        if ["https://", "http://", "mailto:"].iter().any(|scheme| {
            uri.get(..scheme.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
                && uri.len() > scheme.len()
        }) && !uri.chars().any(char::is_control)
        {
            return Ok(Some(format!("href=\"{}\"", escape(uri))));
        }
    }
    Ok(None)
}

fn href(
    positions: &Positions,
    emitted_offsets: &Offsets,
    id: u64,
    offset: usize,
) -> Option<String> {
    positions.get(&id).map(|section| {
        let mut target = format!("section-{section}.xhtml#kfx-{id}");
        if emitted_offsets
            .get(&id)
            .is_some_and(|offsets| offsets.contains(&offset))
        {
            let _ = write!(target, "-{offset}");
        }
        target
    })
}

pub fn resolve_links(mut body: &str, positions: &Positions, emitted_offsets: &Offsets) -> String {
    const MARKER: &str = " data-kfx-target=\"";
    let mut resolved = String::with_capacity(body.len());
    while let Some((before, rest)) = body.split_once(MARKER) {
        resolved.push_str(before);
        let Some((id, after)) = rest.split_once('"') else {
            break;
        };
        if let Some((id, offset)) = id.split_once(':')
            && let (Ok(id), Ok(offset)) = (id.parse(), offset.parse())
            && let Some(target) = href(positions, emitted_offsets, id, offset)
        {
            let _ = write!(resolved, " href=\"{target}\"");
        }
        body = after;
    }
    resolved.push_str(body);
    resolved
}

fn resolve_record<'a>(book: &'a Book, value: Value<'a>, kind: u64) -> Result<Option<Value<'a>>> {
    match value.kind {
        13 => Ok(Some(value)),
        7 => {
            let id = number(value, 7)?;
            if book.fragments.contains_key(&(kind, id)) {
                return Ok(Some(book.fragment(kind, id)?));
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn render_entries(
    book: &Book,
    positions: &Positions,
    emitted_offsets: &Offsets,
    items: &[u8],
    depth: usize,
    count: &mut usize,
) -> Result<String> {
    ensure!(depth < 32, "KFX navigation exceeds nesting limit");
    let mut output = String::new();
    for item in entries(items) {
        *count += 1;
        ensure!(*count <= 10_000, "KFX navigation exceeds entry limit");
        let Some(unit) = resolve_record(book, item?, 393)? else {
            continue;
        };
        let children = match field(unit.body, 247)? {
            Some(items) => render_entries(
                book,
                positions,
                emitted_offsets,
                list(items)?,
                depth + 1,
                count,
            )?,
            None => String::new(),
        };
        let label = match field(unit.body, 241)? {
            Some(representation) => field(representation.body, 244)?
                .filter(|value| value.kind == 8)
                .map(string)
                .transpose()?,
            None => None,
        };
        let target = match field(unit.body, 246)? {
            Some(location) => {
                let offset = offset(location.body)?;
                position(location.body)?.and_then(|id| href(positions, emitted_offsets, id, offset))
            }
            None => None,
        };
        if let (Some(label), Some(target)) =
            (label.filter(|label| !label.trim().is_empty()), target)
        {
            write!(output, "<li><a href=\"{target}\">{}</a>", escape(label))?;
            if !children.is_empty() {
                write!(output, "<ol>{children}</ol>")?;
            }
            output.push_str("</li>\n");
        } else {
            output.push_str(&children);
        }
    }
    Ok(output)
}

pub fn contents(
    book: &Book,
    positions: &Positions,
    emitted_offsets: &Offsets,
) -> Result<Option<String>> {
    let mut ids: Vec<_> = book
        .fragments
        .keys()
        .filter_map(|&(kind, id)| (kind == 389).then_some(id))
        .collect();
    ids.sort_unstable();
    for id in ids {
        for order in entries(list(book.fragment(389, id)?)?) {
            let Some(containers) = field(order?.body, 392)? else {
                continue;
            };
            for container in entries(list(containers)?) {
                let Some(container) = resolve_record(book, container?, 391)? else {
                    continue;
                };
                let Some(kind) = field(container.body, 235)? else {
                    continue;
                };
                if kind.kind != 7 || number(kind, 7)? != 212 {
                    continue;
                }
                if let Some(items) = field(container.body, 247)? {
                    let nav =
                        render_entries(book, positions, emitted_offsets, list(items)?, 0, &mut 0)?;
                    if !nav.is_empty() {
                        return Ok(Some(nav));
                    }
                }
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_links_point_to_the_section_containing_the_anchor() {
        let body =
            "<a data-kfx-target=\"21:2\">Chapter</a> <a data-kfx-target=\"99:0\">Missing</a>";
        let positions = Positions::from([(21, 3)]);
        assert_eq!(
            resolve_links(
                body,
                &positions,
                &Offsets::from([(21, BTreeSet::from([2]))])
            ),
            "<a href=\"section-3.xhtml#kfx-21-2\">Chapter</a> <a>Missing</a>"
        );
    }
}
