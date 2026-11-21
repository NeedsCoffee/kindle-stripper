use crate::{
    epub::{entries, escape},
    ion::Value,
    kfx_pdf::{Book, field, list, number, required},
};
use anyhow::{Result, ensure};
use std::{collections::BTreeMap, fmt::Write};

type Declarations = BTreeMap<&'static str, String>;

fn signed_bytes(bytes: &[u8]) -> Option<f64> {
    let (&first, tail) = bytes.split_first()?;
    if bytes.len() > 8 {
        return None;
    }
    let magnitude = tail.iter().fold(u64::from(first & 127), |n, &byte| {
        (n << 8) | u64::from(byte)
    });
    Some(if first & 128 != 0 {
        -(magnitude as f64)
    } else {
        magnitude as f64
    })
}

fn scalar(value: Value<'_>) -> Option<f64> {
    match value.kind {
        2 | 3 => Some(
            number(Value { kind: 2, ..value }, 2).ok()? as f64
                * if value.kind == 3 { -1.0 } else { 1.0 },
        ),
        4 => match value.body.len() {
            0 => Some(0.0),
            4 => Some(f64::from(f32::from_be_bytes(value.body.try_into().ok()?))),
            8 => Some(f64::from_be_bytes(value.body.try_into().ok()?)),
            _ => None,
        },
        5 => {
            if value.body.is_empty() {
                return Some(0.0);
            }
            let (&first, mut rest) = value.body.split_first()?;
            let mut exponent = i32::from(first & 63);
            let mut current = first;
            while current & 128 == 0 {
                (current, rest) = rest.split_first().map(|(&byte, tail)| (byte, tail))?;
                exponent = exponent
                    .checked_mul(128)?
                    .checked_add(i32::from(current & 127))?;
            }
            if first & 64 != 0 {
                exponent = -exponent;
            }
            Some(signed_bytes(rest).unwrap_or(0.0) * 10f64.powi(exponent))
        }
        _ => None,
    }
}

fn measurement(value: Value<'_>, allow_unitless: bool, allow_negative: bool) -> Option<String> {
    let (amount, unit) = if value.kind == 13 {
        let amount = scalar(field(value.body, 307).ok()??)?;
        let unit = match number(field(value.body, 306).ok()??, 7).ok()? {
            308 if allow_unitless => "",
            308 => "em",
            309 => "ex",
            314 => "%",
            318 => "pt",
            319 => "px",
            505 => "rem",
            506 => "ch",
            _ => return None,
        };
        (amount, unit)
    } else {
        let amount = scalar(value)?;
        if amount != 0.0 && !allow_unitless {
            return None;
        }
        (amount, "")
    };
    if !amount.is_finite() || amount.abs() > 10_000.0 || (!allow_negative && amount < 0.0) {
        return None;
    }
    Some(format!("{amount:.5}{unit}"))
}

fn declaration(id: usize, value: Value<'_>, inline: bool) -> Option<(&'static str, String)> {
    let keyword = |choices: &[(u64, &'static str)]| {
        let sid = number(Value { ..value }, 7).ok()?;
        choices
            .iter()
            .find_map(|&(symbol, css)| (symbol == sid).then(|| css.to_owned()))
    };
    let (name, css) = match id {
        12 => (
            "font-style",
            keyword(&[(350, "normal"), (381, "oblique"), (382, "italic")]),
        ),
        13 => (
            "font-weight",
            keyword(&[
                (350, "normal"),
                (355, "100"),
                (356, "200"),
                (357, "300"),
                (359, "500"),
                (360, "600"),
                (361, "bold"),
                (362, "800"),
                (363, "900"),
            ]),
        ),
        16 => ("font-size", measurement(Value { ..value }, false, false)),
        23 => (
            "text-decoration",
            keyword(&[
                (328, "underline"),
                (329, "underline double"),
                (330, "underline dashed"),
                (331, "underline dotted"),
                (349, "none"),
            ]),
        ),
        44 => (
            "vertical-align",
            keyword(&[(350, "baseline"), (370, "super"), (371, "sub")]),
        ),
        583 => (
            "font-variant",
            keyword(&[(349, "normal"), (369, "small-caps")]),
        ),
        34 if !inline => (
            "text-align",
            keyword(&[
                (59, "left"),
                (61, "right"),
                (320, "center"),
                (321, "justify"),
            ]),
        ),
        36 if !inline => ("text-indent", measurement(Value { ..value }, false, true)),
        42 if !inline => (
            "line-height",
            keyword(&[(383, "normal")]).or_else(|| measurement(Value { ..value }, true, false)),
        ),
        47..=50 if !inline => (
            ["margin-top", "margin-left", "margin-bottom", "margin-right"][id - 47],
            measurement(Value { ..value }, false, true),
        ),
        _ => return None,
    };
    Some((name, css?))
}

fn collect(mut fields: &[u8], inline: bool, declarations: &mut Declarations) -> Result<()> {
    while !fields.is_empty() {
        let (id, value) = crate::ion::field(&mut fields)?;
        if let Some((name, css)) = declaration(id, value, inline) {
            declarations.insert(name, css);
        }
    }
    Ok(())
}

fn declarations(book: &Book, fields: &[u8], inline: bool) -> Result<Declarations> {
    let mut result = Declarations::new();
    if let Some(style) = field(fields, 157)? {
        let id = number(style, 7)?;
        if book.fragments.contains_key(&(157, id)) {
            collect(book.fragment(157, id)?.body, inline, &mut result)?;
        }
    }
    collect(fields, inline, &mut result)?;
    Ok(result)
}

pub fn image_style(book: &Book, fields: &[u8]) -> Result<String> {
    let mut css = Declarations::new();
    let style = field(fields, 157)?
        .map(|style| number(style, 7))
        .transpose()?;
    let named = style
        .filter(|id| book.fragments.contains_key(&(157, *id)))
        .map(|id| book.fragment(157, id))
        .transpose()?;
    let size = |id| -> Result<Option<String>> {
        let direct = field(fields, id)?;
        let value = match (direct, named.as_ref()) {
            (Some(value), _) => Some(value),
            (None, Some(style)) => field(style.body, id)?,
            (None, None) => None,
        };
        Ok(value.and_then(|value| measurement(value, false, false)))
    };
    // Treat KFX dimensions as ceilings. This retains the published size of
    // high-resolution icons without stretching low-resolution images.
    let widths = [size(56)?, size(65)?];
    let widths: Vec<_> = widths.iter().flatten().map(String::as_str).collect();
    if !widths.is_empty() {
        css.insert("max-width", format!("min(100%,{})", widths.join(",")));
    }
    let heights = [size(57)?, size(64)?];
    let heights: Vec<_> = heights.iter().flatten().map(String::as_str).collect();
    if let [height] = heights.as_slice() {
        css.insert("max-height", (*height).to_owned());
    } else if !heights.is_empty() {
        css.insert("max-height", format!("min({})", heights.join(",")));
    }
    Ok(style_attribute(&css))
}

fn style_attribute(css: &Declarations) -> String {
    if css.is_empty() {
        return String::new();
    }
    let mut attribute = String::from(" style=\"");
    for (name, value) in css {
        let _ = write!(attribute, "{name}:{value};");
    }
    attribute.push('"');
    attribute
}

fn text_markup(text: &str) -> String {
    escape(text)
        .replace("\r\n", "<br/>")
        .replace(['\r', '\n'], "<br/>")
}

pub fn alignment_style(book: &Book, fields: &[u8]) -> Result<String> {
    let mut css = declarations(book, fields, false)?;
    css.retain(|name, _| *name == "text-align");
    // KFX scale-fit containers can carry a separate horizontal centering hint.
    if field(fields, 156)?.and_then(|value| number(value, 7).ok()) == Some(326)
        && field(fields, 140)?.and_then(|value| number(value, 7).ok()) == Some(320)
    {
        css.entry("text-align")
            .or_insert_with(|| "center".to_owned());
    }
    Ok(style_attribute(&css))
}

pub fn block_style(book: &Book, fields: &[u8]) -> Result<String> {
    Ok(style_attribute(&declarations(book, fields, false)?))
}

pub fn paragraph(
    book: &Book,
    fields: &[u8],
    text: &str,
    anchors: &[(usize, u64)],
    output: &mut String,
) -> Result<()> {
    let block_style = block_style(book, fields)?;
    let block_link = crate::epub_navigation::link_attribute(book, fields)?;
    write!(output, "<p{block_style}>")?;
    let events = field(fields, 142)?
        .map(list)
        .transpose()?
        .unwrap_or_default();
    if !events.is_empty() || !anchors.is_empty() {
        linked_text(book, events, text, block_link.as_deref(), anchors, output)?;
    } else {
        write_text(output, text, &Declarations::new(), block_link.as_deref())?;
    }
    if text.is_empty() {
        output.push_str("<br/>");
    }
    output.push_str("</p>\n");
    Ok(())
}

fn write_text(
    output: &mut String,
    text: &str,
    css: &Declarations,
    link: Option<&str>,
) -> Result<()> {
    if let Some(link) = link {
        write!(output, "<a {link}>")?;
    }
    let chunk = text_markup(text);
    if css.is_empty() {
        output.push_str(&chunk);
    } else {
        write!(output, "<span{}>{chunk}</span>", style_attribute(css))?;
    }
    if link.is_some() {
        output.push_str("</a>");
    }
    Ok(())
}

fn linked_text(
    book: &Book,
    events: &[u8],
    text: &str,
    block_link: Option<&str>,
    anchors: &[(usize, u64)],
    output: &mut String,
) -> Result<()> {
    // KFX ranges count Unicode characters. Byte boundaries keep slices valid
    // for multibyte text while preserving the original offset and length.
    let offsets: Vec<_> = text
        .char_indices()
        .map(|(index, _)| index)
        .chain([text.len()])
        .collect();
    let mut ranges = Vec::new();
    for (event_index, event) in entries(events).enumerate() {
        ensure!(event_index < 4096, "too many KFX style ranges");
        let event = event?;
        let css = declarations(book, event.body, true)?;
        let link = crate::epub_navigation::link_attribute(book, event.body)?;
        if css.is_empty() && link.is_none() {
            continue;
        }
        let start = usize::try_from(number(required(event.body, 143)?, 2)?)?;
        let length = usize::try_from(number(required(event.body, 144)?, 2)?)?;
        let end = start.saturating_add(length).min(offsets.len() - 1);
        if start < end {
            ranges.push((offsets[start], offsets[end], css, link));
        }
    }
    let mut cuts = vec![0, text.len()];
    for &(start, end, _, _) in &ranges {
        cuts.extend([start, end]);
    }
    let anchors: BTreeMap<_, _> = anchors
        .iter()
        .filter_map(|&(offset, id)| offsets.get(offset).map(|&byte| (byte, (offset, id))))
        .collect();
    cuts.extend(anchors.keys());
    cuts.sort_unstable();
    cuts.dedup();
    for bounds in cuts.windows(2) {
        let (start, end) = (bounds[0], bounds[1]);
        if let Some(&(offset, id)) = anchors.get(&start) {
            write!(output, "<span id=\"kfx-{id}-{offset}\"></span>")?;
        }
        let mut combined = Declarations::new();
        let mut link = block_link;
        for (from, to, css, event_link) in &ranges {
            if *from <= start && end <= *to {
                link = event_link.as_deref().or(link);
                for (&name, value) in css {
                    combined.insert(name, value.clone());
                }
            }
        }
        write_text(output, &text[start..end], &combined, link)?;
    }
    if let Some(&(offset, id)) = anchors.get(&text.len()) {
        write!(output, "<span id=\"kfx-{id}-{offset}\"></span>")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_character_ranges_preserve_unicode_and_line_breaks() {
        // Range 0..3 is italic. Range 1..2 is also bold.
        let events = [
            0xd9, 0x01, 0x8f, 0x20, 0x01, 0x90, 0x21, 3, 0x8c, 0x72, 0x01, 0x7e,
        ];
        // Encode the two Ion structs as field-name/value pairs.
        let italic = [
            0xdb, 0x01, 0x8f, 0x20, 0x01, 0x90, 0x21, 3, 0x8c, 0x72, 0x01, 0x7e,
        ];
        let bold = [
            0xdc, 0x01, 0x8f, 0x21, 1, 0x01, 0x90, 0x21, 1, 0x8d, 0x72, 0x01, 0x69,
        ];
        let mut output = String::new();
        assert!(linked_text(&Book::default(), &events, "Aé&\n", None, &[], &mut output).is_err());
        output.clear();
        linked_text(
            &Book::default(),
            &[italic.as_slice(), bold.as_slice()].concat(),
            "Aé&\n",
            None,
            &[(1, 21)],
            &mut output,
        )
        .unwrap();
        assert_eq!(
            output,
            concat!(
                "<span style=\"font-style:italic;\">A</span>",
                "<span id=\"kfx-21-1\"></span>",
                "<span style=\"font-style:italic;font-weight:bold;\">é</span>",
                "<span style=\"font-style:italic;\">&amp;</span><br/>",
            )
        );
    }
}
