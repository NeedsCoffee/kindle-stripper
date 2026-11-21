use anyhow::{Context, Result, ensure};
use lopdf::{Dictionary, Document, Object, Stream, dictionary};

pub fn import(target: &mut Document, bytes: &[u8]) -> Result<Vec<Dictionary>> {
    let mut source = Document::load_mem(bytes).context("reading embedded PDF")?;
    ensure!(!source.is_encrypted(), "embedded PDF is encrypted");
    ensure!(
        source.objects.len() <= 100_000,
        "embedded PDF has too many objects"
    );
    source.renumber_objects_with(
        target
            .max_id
            .checked_add(1)
            .context("PDF object ID overflow")?,
    );
    let mut pages = Vec::new();
    for id in source.get_pages().into_values() {
        let mut page = source.get_dictionary(id)?.clone();
        let mut ancestor = id;
        // Flatten inheritable page attributes before replacing the page tree.
        for depth in 0..64 {
            let dictionary = source.get_dictionary(ancestor)?;
            for key in [b"Resources".as_slice(), b"MediaBox", b"CropBox", b"Rotate"] {
                if !page.has(key)
                    && let Ok(value) = dictionary.get(key)
                {
                    page.set(key, value.clone());
                }
            }
            let Ok(parent) = dictionary.get(b"Parent") else {
                break;
            };
            ensure!(depth < 63, "invalid or too deeply nested PDF page tree");
            ancestor = parent.as_reference()?;
        }
        ensure!(page.has(b"MediaBox"), "embedded PDF page has no media box");
        // Annotations can reference pages in the source tree. The merged
        // document keeps the page artwork and leaves those links behind.
        page.remove(b"Annots");
        page.remove(b"AA");
        pages.push(page);
    }
    ensure!(!pages.is_empty(), "embedded PDF has no pages");
    ensure!(
        target.objects.len().saturating_add(source.objects.len()) <= 300_000,
        "merged PDF exceeds object limit"
    );
    target.max_id = target.max_id.max(source.max_id);
    target.objects.extend(source.objects);
    Ok(pages)
}

pub fn jpeg(target: &mut Document, bytes: Vec<u8>) -> Result<Vec<Dictionary>> {
    let (width, height, colors) = jpeg_dimensions(&bytes)?;
    let image = target.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Filter" => "DCTDecode",
            "Width" => i64::from(width), "Height" => i64::from(height),
            "BitsPerComponent" => 8, "ColorSpace" => colors
        },
        bytes,
    ));
    let content = target.add_object(Stream::new(
        Dictionary::new(),
        format!("q {width} 0 0 {height} 0 0 cm /Im0 Do Q\n").into_bytes(),
    ));
    Ok(vec![dictionary! {
        "Type" => "Page",
        "MediaBox" => vec![Object::Integer(0), Object::Integer(0), width.into(), height.into()],
        "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image } },
        "Contents" => content
    }])
}

fn jpeg_dimensions(bytes: &[u8]) -> Result<(u16, u16, &'static str)> {
    let mut data = bytes
        .strip_prefix(b"\xff\xd8")
        .context("unsupported KFX page media")?;
    while let Some(markers) = data.strip_prefix(b"\xff") {
        let skip = markers.iter().take_while(|&&byte| byte == 0xff).count();
        let (&marker, segment) = markers[skip..]
            .split_first()
            .context("truncated JPEG marker")?;
        ensure!(
            !matches!(marker, 0xda | 0xd9),
            "JPEG has no supported frame"
        );
        let length = u16::from_be_bytes(
            segment
                .get(..2)
                .context("truncated JPEG segment")?
                .try_into()?,
        );
        let (body, tail) = segment
            .split_at_checked(usize::from(length))
            .context("truncated JPEG segment")?;
        ensure!(length >= 2, "invalid JPEG segment");
        if matches!(marker, 0xc0..=0xc2) {
            ensure!(body.len() >= 8 && body[2] == 8, "unsupported JPEG frame");
            let height = u16::from_be_bytes(body[3..5].try_into()?);
            let width = u16::from_be_bytes(body[5..7].try_into()?);
            let colors = match body[7] {
                1 => "DeviceGray",
                3 => "DeviceRGB",
                _ => anyhow::bail!("unsupported JPEG color space"),
            };
            ensure!(width > 0 && height > 0, "invalid JPEG dimensions");
            return Ok((width, height, colors));
        }
        data = tail;
    }
    anyhow::bail!("invalid JPEG frame")
}
