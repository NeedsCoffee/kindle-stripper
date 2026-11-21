//! Repeatable mutation tests for the native book parsers.

use crate::{drmion, ion};

pub fn exercise(seed: &[u8], mut check: impl FnMut(&[u8])) {
    for length in 0..=seed.len() {
        check(&seed[..length]);
    }
    for offset in 0..seed.len() {
        let mut input = seed.to_vec();
        for byte in [0, 0x7f, 0x80, 0xff] {
            input[offset] = byte;
            check(&input);
        }
    }

    let mut state = 0x4b46_5821_u32;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state as usize
    };
    for _ in 0..10_000 {
        let mut input = seed.to_vec();
        for _ in 0..(1 + next() % 8) {
            let position = next() % (input.len() + 1);
            let byte = next() as u8;
            match next() % 3 {
                0 if position < input.len() => input[position] ^= byte,
                1 if position < input.len() => {
                    input.remove(position);
                }
                _ => input.insert(position, byte),
            }
        }
        check(&input);

        let noise: Vec<_> = (0..next() % 256).map(|_| next() as u8).collect();
        check(&noise);
    }
}

fn varuint(mut number: usize) -> Vec<u8> {
    let mut bytes = vec![(number as u8 & 0x7f) | 0x80];
    number >>= 7;
    while number != 0 {
        bytes.push(number as u8 & 0x7f);
        number >>= 7;
    }
    bytes.reverse();
    bytes
}

pub fn value(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut result = if body.len() >= 14 || kind == 13 && body.len() == 1 {
        let mut descriptor = vec![(kind << 4) | 14];
        descriptor.extend(varuint(body.len()));
        descriptor
    } else {
        vec![(kind << 4) | body.len() as u8]
    };
    result.extend(body);
    result
}

pub fn field(sid: usize, kind: u8, body: &[u8]) -> Vec<u8> {
    let mut result = varuint(sid);
    result.extend(value(kind, body));
    result
}

pub fn annotated(sid: usize, inner: &[u8]) -> Vec<u8> {
    let sid = varuint(sid);
    let mut body = varuint(sid.len());
    body.extend(sid);
    body.extend(inner);
    value(14, &body)
}

#[test]
fn fuzz_ion_values_and_fields() {
    let seed = field(24, 11, &annotated(40, &value(10, b"example")));
    assert!(ion::field(&mut seed.as_slice()).is_ok());
    exercise(&seed, |mut input| {
        while !input.is_empty() {
            let mut remaining = input;
            if ion::value(&mut remaining).is_err() {
                break;
            }
            assert!(remaining.len() < input.len());
            input = remaining;
        }
    });
    exercise(&seed, |input| {
        let mut remaining = input;
        if ion::field(&mut remaining).is_ok() {
            assert!(remaining.len() < input.len());
        }
        let _ = ion::protected_data_import(input);
    });
}

#[test]
fn fuzz_drmion_envelopes() {
    let page = annotated(40, &value(13, &field(24, 10, b"CONTsynthetic")));
    let seed = drmion_seed(&page);
    let keys = drmion::ContentKeys::new();
    assert_eq!(drmion::decrypt(&seed, &keys).unwrap(), b"CONTsynthetic");
    exercise(&seed, |input| {
        if let Ok(container) = drmion::decrypt(input, &keys) {
            assert!(container.starts_with(b"CONT"));
        }
    });
}

#[test]
fn drmion_requires_end_marker_after_pages() {
    let page = annotated(40, &value(13, &field(24, 10, b"CONTsynthetic")));
    let mut input = drmion_seed(&page);
    let trailer_start = input.len() - 8;
    let marker = annotated(52, &[0x70]);
    input.drain(trailer_start - marker.len()..trailer_start);
    let error = drmion::decrypt(&input, &drmion::ContentKeys::new()).unwrap_err();
    assert!(error.to_string().contains("missing DRMION end marker"));
}

#[test]
fn fuzz_drmion_compressed_page_payload() {
    let plaintext = b"CONTsynthetic compressed page";
    let mut compressed = Vec::new();
    lzma_rs::lzma_compress(&mut plaintext.as_slice(), &mut compressed).unwrap();
    // Keep the LZMA header fixed so the campaign targets the compressed bytes
    // with the size and dictionary settings of this small fixture.
    compressed[5..13].copy_from_slice(&(plaintext.len() as u64).to_le_bytes());
    let (header, payload) = compressed.split_at(13);
    let keys = drmion::ContentKeys::new();
    let wrap = |payload: &[u8]| {
        let mut data = vec![0];
        data.extend(header);
        data.extend(payload);
        let mut contents = varuint(24);
        contents.extend(annotated(72, &value(10, &data)));
        drmion_seed(&annotated(40, &value(13, &contents)))
    };
    assert_eq!(drmion::decrypt(&wrap(payload), &keys).unwrap(), plaintext);
    exercise(payload, |input| {
        if let Ok(container) = drmion::decrypt(&wrap(input), &keys) {
            assert!(container.starts_with(b"CONT"));
        }
    });
}

fn drmion_seed(page: &[u8]) -> Vec<u8> {
    let import = value(13, &field(4, 8, b"ProtectedData"));
    let table = annotated(3, &value(13, &field(6, 11, &import)));
    let mut seed = drmion::MAGIC.to_vec();
    seed.extend(b"\xe0\x01\x00\xea");
    seed.extend(table);
    seed.extend(annotated(50, &[0x70]));
    seed.extend(annotated(10, &value(11, page)));
    seed.extend(annotated(52, &[0x70]));
    seed.extend([0; 8]);
    seed
}
