// SPDX-License-Identifier: GPL-3.0-or-later
//! The DLLs a Windows program names (PE32+ import tables): those loaded with the program, and
//! those loaded on first use (`/DELAYLOAD`).

use anyhow::{Context, Result, bail};

/// The DLL names of the import table and of the delay-load import table.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Imports {
    pub at_start: Vec<String>,
    pub delayed: Vec<String>,
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16> {
    let b = bytes.get(at..at + 2).context("truncated PE file")?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32> {
    let b = bytes.get(at..at + 4).context("truncated PE file")?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// A section: `(virtual address, virtual size, file offset)`.
type Section = (u32, u32, u32);

/// Where the optional header starts, and the sections.
fn sections(bytes: &[u8]) -> Result<(usize, Vec<Section>)> {
    if bytes.get(..2) != Some(b"MZ") {
        bail!("not a Windows program");
    }
    let pe = u32_at(bytes, 0x3c)? as usize;
    if bytes.get(pe..pe + 4) != Some(b"PE\0\0") {
        bail!("no PE header");
    }
    let coff = pe + 4;
    let count = usize::from(u16_at(bytes, coff + 2)?);
    let optional = coff + 20;
    let first = optional + usize::from(u16_at(bytes, coff + 16)?);
    let mut list = Vec::with_capacity(count);
    for i in 0..count {
        let s = first + i * 40;
        let size = u32_at(bytes, s + 8)?.max(u32_at(bytes, s + 16)?);
        list.push((u32_at(bytes, s + 12)?, size, u32_at(bytes, s + 20)?));
    }
    Ok((optional, list))
}

fn offset(sections: &[Section], rva: u32) -> Result<usize> {
    sections
        .iter()
        .find(|(va, size, _)| rva >= *va && rva - va < *size)
        .map(|(va, _, raw)| (rva - va + raw) as usize)
        .with_context(|| format!("address {rva:#x} is in no section"))
}

fn name_at(bytes: &[u8], sections: &[Section], rva: u32) -> Result<String> {
    let start = offset(sections, rva)?;
    let tail = bytes.get(start..).context("truncated PE file")?;
    let end = tail
        .iter()
        .position(|b| *b == 0)
        .context("unterminated name")?;
    Ok(String::from_utf8_lossy(&tail[..end]).into_owned())
}

/// The names in the descriptor table of data directory `index` (`stride` bytes per entry, the
/// name's address `name` bytes in); an all-zero entry ends it.
fn table(
    bytes: &[u8],
    optional: usize,
    sections: &[Section],
    (index, stride, name): (usize, usize, usize),
) -> Result<Vec<String>> {
    let directories = optional + 112;
    if index >= u32_at(bytes, optional + 108)? as usize {
        return Ok(Vec::new());
    }
    let rva = u32_at(bytes, directories + index * 8)?;
    if rva == 0 {
        return Ok(Vec::new());
    }
    let mut at = offset(sections, rva)?;
    let mut names = Vec::new();
    loop {
        let entry = bytes
            .get(at..at + stride)
            .context("truncated import table")?;
        if entry.iter().all(|b| *b == 0) {
            return Ok(names);
        }
        names.push(name_at(bytes, sections, u32_at(bytes, at + name)?)?);
        at += stride;
    }
}

pub fn imports(bytes: &[u8]) -> Result<Imports> {
    let (optional, sections) = sections(bytes)?;
    if u16_at(bytes, optional)? != 0x20b {
        bail!("not a 64-bit program");
    }
    Ok(Imports {
        at_start: table(bytes, optional, &sections, (1, 20, 12))?,
        delayed: table(bytes, optional, &sections, (13, 32, 4))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put32(bytes: &mut [u8], at: usize, v: u32) {
        bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// A program with one section (address 0x1000, file offset 0x200) holding both tables.
    fn program(at_start: &[&str], delayed: &[&str]) -> Vec<u8> {
        let mut b = vec![0_u8; 0x1000];
        b[..2].copy_from_slice(b"MZ");
        put32(&mut b, 0x3c, 0x80);
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        let coff = 0x84;
        b[coff + 2] = 1; // one section
        b[coff + 16] = 240; // optional header size (PE32+, 16 directories)
        let optional = coff + 20;
        b[optional..optional + 2].copy_from_slice(&0x20b_u16.to_le_bytes());
        put32(&mut b, optional + 108, 16);
        let section = optional + 240;
        put32(&mut b, section + 8, 0xe00);
        put32(&mut b, section + 12, 0x1000);
        put32(&mut b, section + 16, 0xe00);
        put32(&mut b, section + 20, 0x200);
        // The tables at 0x1000 (imports) and 0x1100 (delay-load), the names from 0x1800 on.
        let raw = |rva: usize| rva - 0x1000 + 0x200;
        let mut text = 0x1800;
        let mut write =
            |b: &mut Vec<u8>, table: usize, stride: usize, name: usize, dlls: &[&str]| {
                for (i, dll) in dlls.iter().enumerate() {
                    b[raw(text)..raw(text) + dll.len()].copy_from_slice(dll.as_bytes());
                    put32(b, raw(table + i * stride + name), text as u32);
                    text += dll.len() + 1;
                }
            };
        write(&mut b, 0x1000, 20, 12, at_start);
        write(&mut b, 0x1100, 32, 4, delayed);
        let directories = optional + 112;
        put32(&mut b, directories + 8, 0x1000);
        put32(&mut b, directories + 13 * 8, 0x1100);
        b
    }

    #[test]
    fn both_import_tables_are_read() -> Result<()> {
        let got = imports(&program(
            &["KERNEL32.dll", "d3d11.dll"],
            &["avcodec-63.dll"],
        ))?;
        assert_eq!(got.at_start, ["KERNEL32.dll", "d3d11.dll"]);
        assert_eq!(got.delayed, ["avcodec-63.dll"]);
        Ok(())
    }

    #[test]
    fn a_program_without_delayed_dlls_has_none() -> Result<()> {
        let got = imports(&program(&["KERNEL32.dll"], &[]))?;
        assert!(got.delayed.is_empty());
        assert!(imports(b"not a program").is_err());
        Ok(())
    }
}
