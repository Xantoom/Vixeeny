// SPDX-License-Identifier: GPL-3.0-or-later
//! The linker's maps of the release programs (`Vixeeny.map`, `vixeeny-app.map`, written by their build scripts): which
//! function is at which address. `dist` gathers them into `Vixeeny-<version>-symbols.zip`, kept with the release, and
//! `cargo xtask symbolize <that archive, or a folder of maps> <crash note>` prints the note with a function's name
//! after each `program.exe+0x…` address. The programs ship without debug information: without the maps of their own
//! build, the addresses of a crash say nothing.

use std::io::Read as _;
use std::path::Path;

use anyhow::{Context, Result, bail};

/// The symbols of a program, by address from the start of the program in memory (what a crash note writes).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Map {
    /// Sorted by address; the names that share an address (identical functions are folded into one) stay together.
    symbols: Vec<(u64, Vec<String>)>,
}

impl Map {
    /// Reads a map as `link.exe` and `lld-link` write it: `0001:00000040  name  0000000140001040  [f] [i] lib:object`
    /// lines, the third field being the address in a program loaded at its preferred address.
    pub fn parse(text: &str) -> Result<Self> {
        let base = text
            .lines()
            .find_map(|line| line.trim().strip_prefix("Preferred load address is "))
            .and_then(|hex| u64::from_str_radix(hex.trim(), 16).ok())
            .context("not a linker map: no preferred load address")?;
        let mut found: Vec<(u64, String)> = text
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let (section, offset) = fields.next()?.split_once(':')?;
                let name = fields.next()?;
                let address = fields.next()?;
                // Section 0 holds the absolute symbols, which are at no address of the program.
                let placed = section.len() == 4 && offset.len() == 8 && address.len() == 16 && section != "0000";
                let address = u64::from_str_radix(address, 16).ok().filter(|_| placed)?;
                Some((address.checked_sub(base)?, name.to_owned()))
            })
            .collect();
        found.sort();
        found.dedup();
        let mut symbols: Vec<(u64, Vec<String>)> = Vec::new();
        for (address, name) in found {
            match symbols.last_mut() {
                Some((last, names)) if *last == address => names.push(name),
                _ => symbols.push((address, vec![name])),
            }
        }
        if symbols.is_empty() {
            bail!("the linker map names no symbol");
        }
        Ok(Self { symbols })
    }

    /// The symbol `offset` is in: its readable name, how far into it, and how many other names share its address.
    pub fn at(&self, offset: u64) -> Option<(String, u64, usize)> {
        let after = self.symbols.partition_point(|(address, _)| *address <= offset);
        let (address, names) = self.symbols.get(after.checked_sub(1)?)?;
        Some((format!("{:#}", rustc_demangle::demangle(&names[0])), offset - address, names.len() - 1))
    }

    /// Whether a symbol's name holds `text` (a map of the right program names its crates).
    pub fn names(&self, text: &str) -> bool {
        self.symbols.iter().any(|(_, names)| names.iter().any(|name| name.contains(text)))
    }
}

/// `Vixeeny.exe` → `Vixeeny.map`.
pub fn map_name(program: &str) -> String {
    format!("{}.map", program.strip_suffix(".exe").unwrap_or(program))
}

/// The map of `program` in `source`, a folder of maps or a symbols archive; `None` when it holds none for it.
fn load(source: &Path, program: &str) -> Result<Option<Map>> {
    let wanted = map_name(program);
    let text = if source.is_dir() {
        let Some(entry) = std::fs::read_dir(source)?
            .flatten()
            .find(|entry| entry.file_name().to_string_lossy().eq_ignore_ascii_case(&wanted))
        else {
            return Ok(None);
        };
        std::fs::read_to_string(entry.path())?
    } else {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(source)?)
            .with_context(|| format!("{} is neither a folder nor an archive", source.display()))?;
        let mut text = None;
        for index in 0..archive.len() {
            let mut file = archive.by_index(index)?;
            if file.name()?.eq_ignore_ascii_case(&wanted) {
                let mut read = String::new();
                file.read_to_string(&mut read)?;
                text = Some(read);
                break;
            }
        }
        let Some(text) = text else {
            return Ok(None);
        };
        text
    };
    Map::parse(&text).with_context(|| wanted).map(Some)
}

/// `line` with ` = function+0x…` after each `program.exe+0x…` address `map_of` has the program of.
fn annotate(line: &str, map_of: &mut impl FnMut(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(plus) = rest.find("+0x") {
        let hex_len = rest[plus + 3..].chars().take_while(char::is_ascii_hexdigit).count();
        let end = plus + 3 + hex_len;
        out.push_str(&rest[..end]);
        let program = rest[..plus].rsplit(|c: char| c.is_whitespace()).next().unwrap_or_default();
        if hex_len > 0
            && program.to_ascii_lowercase().ends_with(".exe")
            && let Some(found) = map_of(&format!("{program}+{}", &rest[plus + 3..end]))
        {
            out.push_str(" = ");
            out.push_str(&found);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// `cargo xtask symbolize <symbols archive or folder of maps> <crash note>`.
pub fn symbolize(args: &[String]) -> Result<()> {
    let [source, note] = args else {
        bail!("usage: cargo xtask symbolize <symbols archive or folder of maps> <crash note>");
    };
    let source = Path::new(source);
    let note = std::fs::read_to_string(note).with_context(|| format!("cannot read {note}"))?;
    let mut maps: Vec<(String, Option<Map>)> = Vec::new();
    let mut failure = None;
    let mut map_of = |address: &str| -> Option<String> {
        let (program, offset) = address.split_once('+')?;
        let key = program.to_ascii_lowercase();
        if !maps.iter().any(|(name, _)| *name == key) {
            let map = load(source, program).unwrap_or_else(|e| {
                failure.get_or_insert(e);
                None
            });
            maps.push((key.clone(), map));
        }
        let map = maps.iter().find(|(name, _)| *name == key)?.1.as_ref()?;
        let (name, into, folded) = map.at(u64::from_str_radix(offset, 16).ok()?)?;
        Some(match folded {
            0 => format!("{name}+0x{into:x}"),
            n => format!("{name}+0x{into:x} (or {n} identical function(s))"),
        })
    };
    let text: Vec<String> = note.lines().map(|line| annotate(line, &mut map_of)).collect();
    if let Some(e) = failure {
        return Err(e);
    }
    println!("{}", text.join("\n"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two linkers' lines: `lld-link` writes no `f`/`i` flags, `link.exe` does.
    const MAP: &str = "\
 vixeeny-app

 Timestamp is 6af26328 (Tue Nov 10 01:19:52 2026)

 Preferred load address is 0000000140000000

 Start         Length     Name                   Class
 0001:00000000 0012d068H .text                   CODE

  Address         Publics by Value              Rva+Base               Lib:Object

 0000:00000000       __guard_fids_count         0000000000000000     <absolute>
 0001:00000040       _RNvCs1234_11vixeeny_app4main 0000000140001040 f   vixeeny_app.o
 0001:00000200       _RNvNtCs1234_11vixeeny_app6record5start 0000000140001200 f i vixeeny_app.o

 entry point at         0001:00000040

 Static symbols

 0001:00000200       folded_twin                0000000140001200     other.o
 0001:00000100       _ZN3std2rt10lang_start17h0123456789abcdefE 0000000140001100     std.o
";

    #[test]
    fn an_address_is_the_function_it_falls_in() {
        let map = Map::parse(MAP).unwrap();
        assert_eq!(map.at(0x1040), Some(("vixeeny_app::main".into(), 0, 0)));
        assert_eq!(map.at(0x10ff), Some(("vixeeny_app::main".into(), 0xbf, 0)));
        assert_eq!(map.at(0x1100), Some(("std::rt::lang_start".into(), 0, 0)));
        // Two names at one address: the first, and how many others.
        assert_eq!(map.at(0x1234), Some(("vixeeny_app::record::start".into(), 0x34, 1)));
        // Before the first function: the headers of the program.
        assert_eq!(map.at(0x10), None);
        assert!(map.names("vixeeny_app") && !map.names("vixeeny_daemon"));
    }

    #[test]
    fn what_is_not_a_map_is_refused() {
        assert!(Map::parse("hello").is_err());
        assert!(Map::parse(" Preferred load address is 0000000140000000\n").is_err());
    }

    #[test]
    fn a_note_gets_a_name_after_each_address_of_a_known_program() {
        let map = Map::parse(MAP).unwrap();
        let mut map_of = |address: &str| {
            let (program, offset) = address.split_once('+')?;
            let (name, into, _) =
                (program == "vixeeny-app.exe").then(|| map.at(u64::from_str_radix(offset, 16).ok()?)).flatten()?;
            Some(format!("{name}+0x{into:x}"))
        };
        assert_eq!(
            annotate("crashed: exception 0xc0000005 (access violation) at vixeeny-app.exe+0x1210", &mut map_of),
            "crashed: exception 0xc0000005 (access violation) at vixeeny-app.exe+0x1210 = vixeeny_app::record::start+0x10"
        );
        assert_eq!(annotate("  nvwgf2umx.dll+0x1040", &mut map_of), "  nvwgf2umx.dll+0x1040");
        assert_eq!(annotate("while reading 0x10", &mut map_of), "while reading 0x10");
        assert_eq!(annotate("thread 4242", &mut map_of), "thread 4242");
    }

    #[test]
    fn the_map_of_a_program_is_named_after_it() {
        assert_eq!(map_name("Vixeeny.exe"), "Vixeeny.map");
        assert_eq!(map_name("vixeeny-app.exe"), "vixeeny-app.map");
    }
}
