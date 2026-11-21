//! Binary Ion subset used by DRMION. Symbol IDs assume the ProtectedData import.

use anyhow::{Context, Result, ensure};

/// A borrowed value body with its binary type code and first annotation SID.
/// The DRMION reader uses the first annotation to distinguish record types.
pub struct Value<'a> {
    pub kind: u8,
    pub annotation: usize,
    pub body: &'a [u8],
}

fn take<'a>(input: &mut &'a [u8], count: usize) -> Result<&'a [u8]> {
    let (value, remainder) = input
        .split_at_checked(count)
        .context("truncated Ion value")?;
    *input = remainder;
    Ok(value)
}

// Ion VarUInt stores seven payload bits per byte, most significant group
// first. The high bit is set on the final byte.
fn varuint(input: &mut &[u8]) -> Result<usize> {
    let mut result = 0usize;
    loop {
        let byte = take(input, 1)?[0];
        result = result.checked_mul(128).context("Ion integer overflow")? | usize::from(byte & 127);
        if byte & 128 != 0 {
            return Ok(result);
        }
    }
}

fn raw_value<'a>(input: &mut &'a [u8]) -> Result<Value<'a>> {
    let descriptor = take(input, 1)?[0];
    let kind = descriptor >> 4;
    // The low nibble usually gives a byte count. Booleans encode their
    // value in that nibble, 15 denotes null, and 14 introduces a VarUInt
    // length. Ordered structs use low nibble 1 with a VarUInt length.
    let size = match (kind, descriptor & 15) {
        (1, _) | (_, 15) => 0,
        (13, 1) | (_, 14) => varuint(input)?,
        (_, size) => usize::from(size),
    };
    Ok(Value {
        kind,
        annotation: 0,
        body: take(input, size)?,
    })
}

// An annotation wrapper contains the annotation byte count, its symbol IDs,
// and exactly one value. Return that value with the first ID attached.
pub fn value<'a>(input: &mut &'a [u8]) -> Result<Value<'a>> {
    let mut item = raw_value(input)?;
    if item.kind == 14 {
        let size = varuint(&mut item.body)?;
        let mut annotations = take(&mut item.body, size)?;
        let annotation = varuint(&mut annotations)?;
        let mut annotated = raw_value(&mut item.body)?;
        ensure!(item.body.is_empty(), "invalid Ion annotation wrapper");
        annotated.annotation = annotation;
        return Ok(annotated);
    }
    Ok(item)
}

// Inside an Ion struct, each value is preceded by a VarUInt field-name SID.
pub fn field<'a>(input: &mut &'a [u8]) -> Result<(usize, Value<'a>)> {
    Ok((varuint(input)?, value(input)?))
}

// The page reader interprets numeric SIDs directly. Its mapping assumes
// ProtectedData is the sole shared import following the system symbols.
pub fn protected_data_import(mut table: &[u8]) -> Result<bool> {
    while !table.is_empty() {
        let (id, imports) = field(&mut table)?;
        if id != 6 || imports.kind != 11 {
            continue;
        }
        let mut entries = imports.body;
        let import = value(&mut entries)?;
        if import.kind != 13 || !entries.is_empty() {
            return Ok(false);
        }
        let mut fields = import.body;
        while !fields.is_empty() {
            let (id, name) = field(&mut fields)?;
            if id == 4 {
                return Ok(name.kind == 8 && name.body == b"ProtectedData");
            }
        }
    }
    Ok(false)
}
