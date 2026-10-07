//! A self-contained x86-64 PE linker for Ryn executables on Windows.
//!
//! It combines the Cranelift object of a program with the precompiled Ryn
//! runtime archive and writes a console executable without `rustc`,
//! `link.exe`, import libraries, or a Windows SDK:
//!
//! - archive members are loaded only when they define a symbol that is still
//!   undefined, the way a traditional linker searches a static library;
//! - COMDAT sections keep their first definition, associative sections follow
//!   their parent, and weak externals fall back to their default symbol;
//! - symbols that remain undefined are imported from the system DLLs that
//!   export them, found by reading those DLLs' export tables at link time;
//!   short import members in the archive name the exact DLL when present;
//! - exception handlers that only `vcruntime140.dll` exports are replaced by
//!   stubs, so executables do not depend on the Visual C++ redistributable;
//!   the runtime is built with `panic=abort` and never unwinds;
//! - `.tls` data and `.CRT$XL*` callbacks produce a TLS directory, `.pdata`
//!   becomes the exception directory, and every absolute address gets a base
//!   relocation so the image supports ASLR.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

const MACHINE_AMD64: u16 = 0x8664;
const IMAGE_BASE: u64 = 0x1_4000_0000;
const SECTION_ALIGN: u32 = 0x1000;
const FILE_ALIGN: u32 = 0x200;

const SCN_CNT_CODE: u32 = 0x20;
const SCN_CNT_INITIALIZED_DATA: u32 = 0x40;
const SCN_CNT_UNINITIALIZED_DATA: u32 = 0x80;
const SCN_LNK_INFO: u32 = 0x200;
const SCN_LNK_REMOVE: u32 = 0x800;
const SCN_LNK_COMDAT: u32 = 0x1000;
const SCN_LNK_NRELOC_OVFL: u32 = 0x0100_0000;
const SCN_MEM_DISCARDABLE: u32 = 0x0200_0000;
const SCN_MEM_EXECUTE: u32 = 0x2000_0000;
const SCN_MEM_READ: u32 = 0x4000_0000;
const SCN_MEM_WRITE: u32 = 0x8000_0000;

const REL_ABSOLUTE: u16 = 0;
const REL_ADDR64: u16 = 1;
const REL_ADDR32: u16 = 2;
const REL_ADDR32NB: u16 = 3;
const REL_REL32: u16 = 4;
const REL_REL32_5: u16 = 9;
const REL_SECTION: u16 = 10;
const REL_SECREL: u16 = 11;

const SYM_EXTERNAL: u8 = 2;
const SYM_STATIC: u8 = 3;
const SYM_LABEL: u8 = 6;
const SYM_WEAK_EXTERNAL: u8 = 105;

const COMDAT_ASSOCIATIVE: u8 = 5;

/// DLLs searched, in order, for symbols the runtime imports.
const SYSTEM_DLLS: &[&str] = &[
    "kernel32.dll",
    "ntdll.dll",
    "ucrtbase.dll",
    "ws2_32.dll",
    "userenv.dll",
    "user32.dll",
    "advapi32.dll",
    "bcrypt.dll",
    "bcryptprimitives.dll",
    "dbghelp.dll",
    "shell32.dll",
    "ole32.dll",
    "kernelbase.dll",
];

/// Exception and C++ runtime helpers that only vcruntime140.dll exports. The
/// runtime aborts on panic, so these are reached only while an exception is
/// dispatched through a frame that has no handler of its own; returning
/// `ExceptionContinueSearch` (1) lets dispatch continue as if absent.
const HANDLER_STUBS: &[&str] = &[
    "__CxxFrameHandler3",
    "__CxxFrameHandler4",
    "__GSHandlerCheck",
    "__GSHandlerCheck_SEH",
    "__GSHandlerCheck_EH",
    "__GSHandlerCheck_EH4",
];

#[derive(Debug)]
pub struct LinkError(pub String);

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn err<T>(message: impl Into<String>) -> Result<T, LinkError> {
    Err(LinkError(message.into()))
}

// ---------------------------------------------------------------------------
// Byte helpers

fn u16_at(data: &[u8], offset: usize) -> Result<u16, LinkError> {
    data.get(offset..offset + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| LinkError("truncated object file".into()))
}

fn u32_at(data: &[u8], offset: usize) -> Result<u32, LinkError> {
    data.get(offset..offset + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| LinkError("truncated object file".into()))
}

fn cstr(data: &[u8], offset: usize) -> String {
    let rest = data.get(offset..).unwrap_or(&[]);
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    String::from_utf8_lossy(&rest[..end]).into_owned()
}

fn align_up(value: u32, align: u32) -> u32 {
    if align <= 1 {
        value
    } else {
        value.div_ceil(align) * align
    }
}

// ---------------------------------------------------------------------------
// COFF objects

#[derive(Clone, Debug)]
struct Section {
    name: String,
    characteristics: u32,
    data: Vec<u8>,
    size: u32,
    relocations: Vec<Relocation>,
    align: u32,
}

#[derive(Clone, Copy, Debug)]
struct Relocation {
    offset: u32,
    symbol: u32,
    kind: u16,
}

#[derive(Clone, Debug)]
struct Symbol {
    name: String,
    value: u32,
    /// 1-based section number; 0 undefined, -1 absolute, -2 debug.
    section: i32,
    class: u8,
    /// Index of the first auxiliary record, when present.
    aux: Option<Vec<u8>>,
}

#[derive(Debug)]
struct Object {
    name: String,
    sections: Vec<Section>,
    /// Indexed by symbol table index; auxiliary slots hold `None`.
    symbols: Vec<Option<Symbol>>,
}

fn parse_object(name: &str, data: &[u8]) -> Result<Object, LinkError> {
    let bigobj = data.len() >= 56
        && u16_at(data, 0)? == 0
        && u16_at(data, 2)? == 0xFFFF
        && u16_at(data, 4)? >= 2;
    let (machine, section_count, symtab, symbol_count, header_size, symbol_size) = if bigobj {
        (
            u16_at(data, 6)?,
            u32_at(data, 44)? as usize,
            u32_at(data, 48)? as usize,
            u32_at(data, 52)? as usize,
            56usize,
            20usize,
        )
    } else {
        let optional = u16_at(data, 16)? as usize;
        (
            u16_at(data, 0)?,
            u16_at(data, 2)? as usize,
            u32_at(data, 8)? as usize,
            u32_at(data, 12)? as usize,
            20 + optional,
            18usize,
        )
    };
    if machine != MACHINE_AMD64 && machine != 0 {
        return err(format!(
            "{name}: not an x86-64 object (machine {machine:#x})"
        ));
    }
    let strings = symtab + symbol_count * symbol_size;
    let long_name = |offset: usize| cstr(data, strings + offset);

    let mut sections = Vec::with_capacity(section_count);
    for index in 0..section_count {
        let base = header_size + index * 40;
        let raw_name = data
            .get(base..base + 8)
            .ok_or_else(|| LinkError(format!("{name}: truncated section table")))?;
        let short = String::from_utf8_lossy(
            &raw_name[..raw_name.iter().position(|&b| b == 0).unwrap_or(8)],
        )
        .into_owned();
        let section_name = if let Some(offset) = short.strip_prefix('/') {
            if let Some(encoded) = offset.strip_prefix('/') {
                // Base-64 offsets for very large string tables.
                let mut value = 0usize;
                for ch in encoded.bytes() {
                    let digit = match ch {
                        b'A'..=b'Z' => ch - b'A',
                        b'a'..=b'z' => ch - b'a' + 26,
                        b'0'..=b'9' => ch - b'0' + 52,
                        b'+' => 62,
                        _ => 63,
                    };
                    value = value * 64 + digit as usize;
                }
                long_name(value)
            } else {
                long_name(offset.parse().unwrap_or(0))
            }
        } else {
            short
        };
        let size = u32_at(data, base + 16)?;
        let raw_offset = u32_at(data, base + 20)? as usize;
        let reloc_offset = u32_at(data, base + 24)? as usize;
        let mut reloc_count = u16_at(data, base + 32)? as usize;
        let characteristics = u32_at(data, base + 36)?;
        let uninitialized = characteristics & SCN_CNT_UNINITIALIZED_DATA != 0;
        let raw = if uninitialized || raw_offset == 0 {
            Vec::new()
        } else {
            data.get(raw_offset..raw_offset + size as usize)
                .ok_or_else(|| {
                    LinkError(format!("{name}: section {section_name} is out of bounds"))
                })?
                .to_vec()
        };
        let mut first = 0;
        if characteristics & SCN_LNK_NRELOC_OVFL != 0 && reloc_count == 0xFFFF {
            reloc_count = u32_at(data, reloc_offset)? as usize;
            first = 1;
        }
        let mut relocations = Vec::with_capacity(reloc_count);
        for r in first..reloc_count {
            let at = reloc_offset + r * 10;
            relocations.push(Relocation {
                offset: u32_at(data, at)?,
                symbol: u32_at(data, at + 4)?,
                kind: u16_at(data, at + 8)?,
            });
        }
        let align_bits = (characteristics >> 20) & 0xF;
        sections.push(Section {
            name: section_name,
            characteristics,
            data: raw,
            size,
            relocations,
            align: if align_bits == 0 {
                16
            } else {
                1 << (align_bits - 1)
            },
        });
    }

    let mut symbols = vec![None; symbol_count];
    let mut index = 0;
    while index < symbol_count {
        let base = symtab + index * symbol_size;
        let record = data
            .get(base..base + symbol_size)
            .ok_or_else(|| LinkError(format!("{name}: truncated symbol table")))?;
        let symbol_name = if record[0..4] == [0, 0, 0, 0] {
            long_name(u32::from_le_bytes([record[4], record[5], record[6], record[7]]) as usize)
        } else {
            String::from_utf8_lossy(
                &record[..record[..8].iter().position(|&b| b == 0).unwrap_or(8)],
            )
            .into_owned()
        };
        let value = u32::from_le_bytes([record[8], record[9], record[10], record[11]]);
        let (section, class, aux_count) = if bigobj {
            (
                i32::from_le_bytes([record[12], record[13], record[14], record[15]]),
                record[18],
                record[19] as usize,
            )
        } else {
            (
                i16::from_le_bytes([record[12], record[13]]) as i32,
                record[16],
                record[17] as usize,
            )
        };
        let aux = (aux_count > 0)
            .then(|| {
                data.get(base + symbol_size..base + symbol_size * 2)
                    .map(|b| b.to_vec())
            })
            .flatten();
        symbols[index] = Some(Symbol {
            name: symbol_name,
            value,
            section,
            class,
            aux,
        });
        index += 1 + aux_count;
    }
    Ok(Object {
        name: name.to_string(),
        sections,
        symbols,
    })
}

// ---------------------------------------------------------------------------
// Archives

enum Member {
    Object(usize),
    Import {
        dll: String,
        name: String,
        data: bool,
    },
}

struct Archive {
    members: Vec<(String, Vec<u8>)>,
    /// External symbol -> member that defines it.
    index: HashMap<String, Member>,
}

fn import_name(symbol: &str, name_type: u16) -> String {
    // 0 ordinal, 1 name, 2 name without prefix (?@_), 3 undecorated.
    match name_type {
        2 => symbol.trim_start_matches(['?', '@', '_']).to_string(),
        3 => symbol
            .trim_start_matches(['?', '@', '_'])
            .split('@')
            .next()
            .unwrap_or(symbol)
            .to_string(),
        _ => symbol.to_string(),
    }
}

fn parse_archive(data: &[u8]) -> Result<Archive, LinkError> {
    if !data.starts_with(b"!<arch>\n") {
        return err("runtime archive is not an ar archive");
    }
    let mut offset = 8;
    let mut long_names: &[u8] = &[];
    let mut members = Vec::new();
    while offset + 60 <= data.len() {
        let header = &data[offset..offset + 60];
        let raw_name = String::from_utf8_lossy(&header[..16])
            .trim_end()
            .to_string();
        let size: usize = String::from_utf8_lossy(&header[48..58])
            .trim()
            .parse()
            .unwrap_or(0);
        let body = data
            .get(offset + 60..offset + 60 + size)
            .ok_or_else(|| LinkError("truncated archive member".into()))?;
        offset += 60 + size + (size & 1);
        if raw_name == "/" || raw_name == "/SYM64/" {
            continue;
        }
        if raw_name == "//" {
            long_names = body;
            continue;
        }
        let name = if let Some(rest) = raw_name.strip_prefix('/') {
            let at: usize = rest.parse().unwrap_or(0);
            let tail = long_names.get(at..).unwrap_or(&[]);
            let end = tail
                .iter()
                .position(|&b| b == 0 || b == b'\n')
                .unwrap_or(tail.len());
            String::from_utf8_lossy(&tail[..end])
                .trim_end_matches('/')
                .to_string()
        } else {
            raw_name.trim_end_matches('/').to_string()
        };
        members.push((name, body.to_vec()));
    }

    let mut index = HashMap::new();
    for (member_index, (name, body)) in members.iter().enumerate() {
        if body.len() >= 20
            && u16_at(body, 0)? == 0
            && u16_at(body, 2)? == 0xFFFF
            && u16_at(body, 4)? == 0
        {
            // Short import object: symbol and DLL names follow the 20-byte header.
            let kind_bits = u16_at(body, 18)?;
            let symbol = cstr(body, 20);
            let dll = cstr(body, 20 + symbol.len() + 1);
            let import_type = kind_bits & 0x3;
            let name_type = (kind_bits >> 2) & 0x7;
            let entry = Member::Import {
                dll: dll.clone(),
                name: import_name(&symbol, name_type),
                data: import_type == 1,
            };
            index.insert(format!("__imp_{symbol}"), entry);
            if import_type == 0 {
                index.insert(
                    symbol.clone(),
                    Member::Import {
                        dll,
                        name: import_name(&symbol, name_type),
                        data: false,
                    },
                );
            }
            continue;
        }
        let object = match parse_object(name, body) {
            Ok(object) => object,
            Err(_) => continue,
        };
        for symbol in object.symbols.iter().flatten() {
            let defined = symbol.section > 0
                || (symbol.section == 0 && symbol.value > 0)
                || symbol.section == -1;
            // Weak externals carry their default definition in the same member
            // (compiler_builtins defines its intrinsics this way).
            if (symbol.class == SYM_EXTERNAL && defined) || symbol.class == SYM_WEAK_EXTERNAL {
                index
                    .entry(symbol.name.clone())
                    .or_insert(Member::Object(member_index));
            }
        }
    }
    Ok(Archive { members, index })
}

// ---------------------------------------------------------------------------
// System DLL exports

fn system_directory() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32")
}

fn dll_exports(path: &Path) -> HashSet<String> {
    let mut names = HashSet::new();
    let Ok(data) = fs::read(path) else {
        return names;
    };
    let mut parse = || -> Result<(), LinkError> {
        let pe = u32_at(&data, 0x3C)? as usize;
        if data.get(pe..pe + 4) != Some(b"PE\0\0") {
            return err("not a PE file");
        }
        let section_count = u16_at(&data, pe + 6)? as usize;
        let optional_size = u16_at(&data, pe + 20)? as usize;
        let optional = pe + 24;
        let magic = u16_at(&data, optional)?;
        let directories = optional + if magic == 0x20B { 112 } else { 96 };
        let export_rva = u32_at(&data, directories)?;
        if export_rva == 0 {
            return Ok(());
        }
        let sections = optional + optional_size;
        let to_offset = |rva: u32| -> Option<usize> {
            (0..section_count).find_map(|i| {
                let s = sections + i * 40;
                let va = u32_at(&data, s + 12).ok()?;
                let size = u32_at(&data, s + 8).ok()?.max(u32_at(&data, s + 16).ok()?);
                let raw = u32_at(&data, s + 20).ok()?;
                (rva >= va && rva < va + size).then(|| (rva - va + raw) as usize)
            })
        };
        let export = to_offset(export_rva).ok_or_else(|| LinkError("bad export table".into()))?;
        let count = u32_at(&data, export + 24)? as usize;
        let names_rva = u32_at(&data, export + 32)?;
        let table = to_offset(names_rva).ok_or_else(|| LinkError("bad export names".into()))?;
        for i in 0..count {
            if let Some(at) = to_offset(u32_at(&data, table + i * 4)?) {
                names.insert(cstr(&data, at));
            }
        }
        Ok(())
    };
    let _ = parse();
    names
}

// ---------------------------------------------------------------------------
// Linking

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum OutKind {
    Text,
    Rdata,
    Data,
    Pdata,
    Tls,
    Bss,
}

impl OutKind {
    fn name(self) -> &'static [u8; 8] {
        match self {
            OutKind::Text => b".text\0\0\0",
            OutKind::Rdata => b".rdata\0\0",
            OutKind::Data => b".data\0\0\0",
            OutKind::Pdata => b".pdata\0\0",
            OutKind::Tls => b".tls\0\0\0\0",
            OutKind::Bss => b".bss\0\0\0\0",
        }
    }

    fn characteristics(self) -> u32 {
        match self {
            OutKind::Text => SCN_CNT_CODE | SCN_MEM_EXECUTE | SCN_MEM_READ,
            OutKind::Rdata | OutKind::Pdata => SCN_CNT_INITIALIZED_DATA | SCN_MEM_READ,
            OutKind::Data | OutKind::Tls => SCN_CNT_INITIALIZED_DATA | SCN_MEM_READ | SCN_MEM_WRITE,
            OutKind::Bss => SCN_CNT_UNINITIALIZED_DATA | SCN_MEM_READ | SCN_MEM_WRITE,
        }
    }
}

/// Where a symbol ends up.
#[derive(Clone, Copy, Debug)]
enum Target {
    /// Object index, section index (0-based), offset within that section.
    Section(usize, usize, u32),
    Absolute(u32),
    /// Index into the synthetic chunk list.
    Synthetic(usize, u32),
    /// An offset within an output section, known only after layout.
    Output(OutKind, u32),
    ImageBase,
}

struct Placed {
    kind: OutKind,
    /// Offset within the output section.
    offset: u32,
}

/// Bytes the linker generates itself (import tables, thunks, stubs, TLS).
struct Chunk {
    kind: OutKind,
    data: Vec<u8>,
    align: u32,
    /// Fixups inside `data` that reference symbols: (offset, kind, symbol name, addend).
    fixups: Vec<(u32, u16, String)>,
    offset: u32,
    placed: bool,
}

fn output_kind(section: &Section) -> Option<OutKind> {
    let base = section.name.split('$').next().unwrap_or("");
    let c = section.characteristics;
    if c & (SCN_LNK_REMOVE | SCN_LNK_INFO) != 0 || c & SCN_MEM_DISCARDABLE != 0 && base != ".reloc"
    {
        return None;
    }
    match base {
        ".debug" | ".drectve" | ".llvm_addrsig" | ".gfids" | ".giats" | ".gljmp" | ".gehcont"
        | ".sxdata" | ".reloc" => None,
        ".pdata" => Some(OutKind::Pdata),
        ".tls" => Some(OutKind::Tls),
        ".xdata" | ".rdata" | ".CRT" | ".edata" => Some(OutKind::Rdata),
        _ if c & SCN_CNT_CODE != 0 || c & SCN_MEM_EXECUTE != 0 => Some(OutKind::Text),
        _ if c & SCN_CNT_UNINITIALIZED_DATA != 0 => Some(OutKind::Bss),
        _ if c & SCN_MEM_WRITE != 0 => Some(OutKind::Data),
        _ => Some(OutKind::Rdata),
    }
}

struct Linker {
    objects: Vec<Object>,
    /// Sections kept per object.
    live: Vec<Vec<bool>>,
    defined: HashMap<String, Target>,
    undefined: BTreeMap<String, ()>,
    weak: HashMap<String, (usize, u32)>,
    comdat_keys: HashSet<String>,
    imports: BTreeMap<String, Vec<(String, bool)>>,
    chunks: Vec<Chunk>,
}

impl Linker {
    fn add_object(&mut self, object: Object) -> Result<(), LinkError> {
        let object_index = self.objects.len();
        let mut live = vec![true; object.sections.len()];
        // COMDAT: the first symbol after the section symbol names the group.
        let mut comdat_parent = vec![None; object.sections.len()];
        let mut section_symbol_seen = vec![false; object.sections.len()];
        for symbol in object.symbols.iter().flatten() {
            if symbol.section <= 0 {
                continue;
            }
            let s = (symbol.section - 1) as usize;
            if s >= object.sections.len()
                || object.sections[s].characteristics & SCN_LNK_COMDAT == 0
            {
                continue;
            }
            if !section_symbol_seen[s] && symbol.class == SYM_STATIC && symbol.value == 0 {
                section_symbol_seen[s] = true;
                if let Some(aux) = &symbol.aux {
                    let selection = aux.get(14).copied().unwrap_or(0);
                    if selection == COMDAT_ASSOCIATIVE {
                        let low = u16::from_le_bytes([aux[12], aux[13]]) as usize;
                        let high = if aux.len() >= 17 {
                            u16::from_le_bytes([aux[15], aux[16]]) as usize
                        } else {
                            0
                        };
                        comdat_parent[s] = Some(low | (high << 16));
                    }
                }
                continue;
            }
            if section_symbol_seen[s] && comdat_parent[s].is_none() && live[s] {
                // First non-section symbol: the COMDAT key.
                if !self.comdat_keys.insert(symbol.name.clone()) && symbol.class == SYM_EXTERNAL {
                    live[s] = false;
                }
                section_symbol_seen[s] = false;
                comdat_parent[s] = Some(usize::MAX);
            }
        }
        // Associative sections live and die with their parent (1-based index).
        for s in 0..object.sections.len() {
            if let Some(parent) = comdat_parent[s]
                && parent != usize::MAX
                && parent >= 1
                && parent <= live.len()
                && !live[parent - 1]
            {
                live[s] = false;
            }
        }
        for (symbol_index, symbol) in object.symbols.iter().enumerate() {
            let Some(symbol) = symbol else { continue };
            if symbol.class == SYM_WEAK_EXTERNAL && symbol.section == 0 {
                if let Some(aux) = &symbol.aux {
                    let tag = u32::from_le_bytes([aux[0], aux[1], aux[2], aux[3]]);
                    self.weak
                        .entry(symbol.name.clone())
                        .or_insert((object_index, tag));
                }
                if !self.defined.contains_key(&symbol.name) {
                    self.undefined.insert(symbol.name.clone(), ());
                }
                continue;
            }
            if symbol.class != SYM_EXTERNAL {
                continue;
            }
            if symbol.section > 0 {
                let s = (symbol.section - 1) as usize;
                if !live.get(s).copied().unwrap_or(false) {
                    continue;
                }
                if self.defined.contains_key(&symbol.name) {
                    let comdat = object.sections[s].characteristics & SCN_LNK_COMDAT != 0;
                    if !comdat {
                        return err(format!(
                            "duplicate symbol `{}` in {}",
                            symbol.name, object.name
                        ));
                    }
                    continue;
                }
                self.defined.insert(
                    symbol.name.clone(),
                    Target::Section(object_index, s, symbol.value),
                );
                self.undefined.remove(&symbol.name);
            } else if symbol.section == -1 {
                self.defined
                    .entry(symbol.name.clone())
                    .or_insert(Target::Absolute(symbol.value));
                self.undefined.remove(&symbol.name);
            } else if symbol.section == 0 && symbol.value > 0 {
                // Common symbol: allocate zeroed storage once.
                if !self.defined.contains_key(&symbol.name) {
                    let chunk = self.chunks.len();
                    self.chunks.push(Chunk {
                        kind: OutKind::Bss,
                        data: vec![0; symbol.value as usize],
                        align: 16,
                        fixups: Vec::new(),
                        offset: 0,
                        placed: false,
                    });
                    self.defined
                        .insert(symbol.name.clone(), Target::Synthetic(chunk, 0));
                    self.undefined.remove(&symbol.name);
                }
            } else if !self.defined.contains_key(&symbol.name) {
                self.undefined.insert(symbol.name.clone(), ());
            }
            let _ = symbol_index;
        }
        self.objects.push(object);
        self.live.push(live);
        Ok(())
    }

    fn synthetic(
        &mut self,
        name: &str,
        kind: OutKind,
        data: Vec<u8>,
        align: u32,
        fixups: Vec<(u32, u16, String)>,
    ) {
        let chunk = self.chunks.len();
        self.chunks.push(Chunk {
            kind,
            data,
            align,
            fixups,
            offset: 0,
            placed: false,
        });
        self.defined
            .insert(name.to_string(), Target::Synthetic(chunk, 0));
        self.undefined.remove(name);
    }
}

/// Links `objects` with the members of `archive` that they need and writes a
/// console executable whose entry point is `entry`.
pub fn link(
    objects: &[(&str, &[u8])],
    archive: &[u8],
    entry: &str,
    output: &Path,
) -> Result<(), LinkError> {
    let archive = parse_archive(archive)?;
    let mut linker = Linker {
        objects: Vec::new(),
        live: Vec::new(),
        defined: HashMap::new(),
        undefined: BTreeMap::new(),
        weak: HashMap::new(),
        comdat_keys: HashSet::new(),
        imports: BTreeMap::new(),
        chunks: Vec::new(),
    };
    linker
        .defined
        .insert("__ImageBase".into(), Target::ImageBase);
    for (name, data) in objects {
        linker.add_object(parse_object(name, data)?)?;
    }
    linker.undefined.insert(entry.to_string(), ());
    if linker.defined.contains_key(entry) {
        linker.undefined.remove(entry);
    }

    // Pull archive members until nothing new resolves.
    let mut loaded = HashSet::new();
    let mut import_members: HashMap<String, (String, String, bool)> = HashMap::new();
    loop {
        let wanted: Vec<String> = linker.undefined.keys().cloned().collect();
        let mut progress = false;
        for name in wanted {
            if linker.defined.contains_key(&name) {
                linker.undefined.remove(&name);
                continue;
            }
            match archive.index.get(&name) {
                Some(Member::Object(member)) if loaded.insert(*member) => {
                    let (member_name, body) = &archive.members[*member];
                    linker.add_object(parse_object(member_name, body)?)?;
                    progress = true;
                }
                Some(Member::Import {
                    dll,
                    name: import,
                    data,
                }) => {
                    import_members.insert(name.clone(), (dll.clone(), import.clone(), *data));
                }
                _ => {}
            }
        }
        if !progress {
            break;
        }
    }

    // Weak externals that stayed undefined take their default definition.
    let weak: Vec<(String, (usize, u32))> =
        linker.weak.iter().map(|(k, v)| (k.clone(), *v)).collect();
    for (name, (object, tag)) in weak {
        if linker.defined.contains_key(&name) {
            continue;
        }
        let default = linker.objects[object]
            .symbols
            .get(tag as usize)
            .cloned()
            .flatten();
        if let Some(default) = default
            && let Some(target) = linker.defined.get(&default.name).copied()
        {
            linker.defined.insert(name.clone(), target);
            linker.undefined.remove(&name);
        }
    }

    // `_tls_index` normally comes from the C runtime's TLS support object.
    let has_tls = linker.objects.iter().zip(&linker.live).any(|(o, live)| {
        o.sections
            .iter()
            .zip(live)
            .any(|(s, &l)| l && output_kind(s) == Some(OutKind::Tls))
    });
    if !linker.defined.contains_key("_tls_index") {
        linker.synthetic("_tls_index", OutKind::Data, vec![0; 8], 8, Vec::new());
    }
    // The vtable of C++ `type_info`, named by Rust's MSVC panic type descriptors.
    // Panics abort, so the descriptors are never inspected.
    if linker.undefined.contains_key("??_7type_info@@6B@") {
        linker.synthetic(
            "??_7type_info@@6B@",
            OutKind::Rdata,
            vec![0; 16],
            8,
            Vec::new(),
        );
    }
    if linker.undefined.contains_key("_fltused") {
        linker.synthetic(
            "_fltused",
            OutKind::Data,
            vec![0x75, 0x98, 0, 0],
            4,
            Vec::new(),
        );
    }
    for stub in HANDLER_STUBS {
        if linker.undefined.contains_key(*stub) {
            // mov eax, 1 ; ret
            linker.synthetic(
                stub,
                OutKind::Text,
                vec![0xB8, 1, 0, 0, 0, 0xC3],
                16,
                Vec::new(),
            );
        }
    }

    // Everything still undefined must come from a DLL.
    let mut exports_cache: Vec<(String, HashSet<String>)> = Vec::new();
    let system = system_directory();
    let mut missing = Vec::new();
    let remaining: Vec<String> = linker.undefined.keys().cloned().collect();
    for symbol in remaining {
        if linker.defined.contains_key(&symbol) {
            continue;
        }
        let (import, data_import) = match symbol.strip_prefix("__imp_") {
            Some(name) => (name.to_string(), true),
            None => (symbol.clone(), false),
        };
        let dll = if let Some((dll, name, _)) = import_members.get(&symbol) {
            Some((dll.to_ascii_lowercase(), name.clone()))
        } else {
            let mut found = None;
            for dll in SYSTEM_DLLS {
                if !exports_cache.iter().any(|(name, _)| name == dll) {
                    exports_cache.push((dll.to_string(), dll_exports(&system.join(dll))));
                }
                let exports = &exports_cache
                    .iter()
                    .find(|(name, _)| name == dll)
                    .unwrap()
                    .1;
                if exports.contains(&import) {
                    found = Some((dll.to_string(), import.clone()));
                    break;
                }
            }
            found
        };
        match dll {
            Some((dll, name)) => {
                linker
                    .imports
                    .entry(dll)
                    .or_default()
                    .push((name, data_import));
                let _ = symbol;
            }
            None => missing.push(symbol),
        }
    }
    if !missing.is_empty() {
        return err(format!("undefined symbols: {}", missing.join(", ")));
    }

    build_imports(&mut linker);
    if has_tls {
        // IMAGE_TLS_DIRECTORY64: raw data start/end, index address, callback array.
        let fixups = vec![
            (0, REL_ADDR64, "__ryn_tls_start".to_string()),
            (8, REL_ADDR64, "__ryn_tls_end".to_string()),
            (16, REL_ADDR64, "_tls_index".to_string()),
            (24, REL_ADDR64, "__ryn_tls_callbacks".to_string()),
        ];
        linker.synthetic(
            "__ryn_tls_directory",
            OutKind::Rdata,
            vec![0; 40],
            8,
            fixups,
        );
    }
    write_image(&mut linker, entry, has_tls, output)
}

/// Creates the import directory, lookup and address tables, hint/name entries,
/// `__imp_` symbols for each IAT slot, and `jmp [__imp_x]` thunks for direct calls.
fn build_imports(linker: &mut Linker) {
    let dlls: Vec<(String, Vec<(String, bool)>)> = linker
        .imports
        .iter()
        .map(|(dll, names)| {
            let mut unique: Vec<String> = names.iter().map(|(n, _)| n.clone()).collect();
            unique.sort();
            unique.dedup();
            (
                dll.clone(),
                unique.into_iter().map(|n| (n, false)).collect(),
            )
        })
        .collect();
    if dlls.is_empty() {
        return;
    }
    // Layout inside one .rdata chunk: descriptors, ILTs, IAT, names.
    let descriptor_size = 20 * (dlls.len() + 1);
    let mut ilt_offsets = Vec::new();
    let mut cursor = descriptor_size;
    for (_, names) in &dlls {
        ilt_offsets.push(cursor);
        cursor += 8 * (names.len() + 1);
    }
    let iat_start = cursor;
    let mut iat_offsets = Vec::new();
    for (_, names) in &dlls {
        iat_offsets.push(cursor);
        cursor += 8 * (names.len() + 1);
    }
    let iat_end = cursor;
    let mut data = vec![0u8; cursor];
    let mut fixups = Vec::new();
    let chunk_name = "__ryn_import_table".to_string();
    let mut name_offsets = Vec::new();
    for (dll, names) in &dlls {
        let mut hints = Vec::new();
        for (name, _) in names {
            let at = data.len();
            if at % 2 == 1 {
                data.push(0);
            }
            let at = data.len();
            data.extend_from_slice(&[0, 0]);
            data.extend_from_slice(name.as_bytes());
            data.push(0);
            hints.push(at);
        }
        let dll_at = data.len();
        data.extend_from_slice(dll.as_bytes());
        data.push(0);
        name_offsets.push((dll_at, hints));
    }
    for (d, (_, names)) in dlls.iter().enumerate() {
        let descriptor = d * 20;
        // OriginalFirstThunk, Name, FirstThunk are RVAs relative to the chunk.
        fixups.push((descriptor as u32, REL_ADDR32NB, chunk_name.clone()));
        data[descriptor..descriptor + 4].copy_from_slice(&(ilt_offsets[d] as u32).to_le_bytes());
        fixups.push((descriptor as u32 + 12, REL_ADDR32NB, chunk_name.clone()));
        data[descriptor + 12..descriptor + 16]
            .copy_from_slice(&(name_offsets[d].0 as u32).to_le_bytes());
        fixups.push((descriptor as u32 + 16, REL_ADDR32NB, chunk_name.clone()));
        data[descriptor + 16..descriptor + 20]
            .copy_from_slice(&(iat_offsets[d] as u32).to_le_bytes());
        for (n, _) in names.iter().enumerate() {
            let hint = name_offsets[d].1[n] as u64;
            for table in [ilt_offsets[d], iat_offsets[d]] {
                let slot = table + n * 8;
                data[slot..slot + 8].copy_from_slice(&hint.to_le_bytes());
                // The high bit stays clear: import by name. Patched to an RVA below.
                fixups.push((slot as u32, REL_ADDR32NB, chunk_name.clone()));
            }
        }
    }
    let chunk = linker.chunks.len();
    linker.chunks.push(Chunk {
        kind: OutKind::Rdata,
        data,
        align: 16,
        fixups,
        offset: 0,
        placed: false,
    });
    linker
        .defined
        .insert(chunk_name, Target::Synthetic(chunk, 0));
    linker.defined.insert(
        "__ryn_iat_start".into(),
        Target::Synthetic(chunk, iat_start as u32),
    );
    linker.defined.insert(
        "__ryn_iat_end".into(),
        Target::Synthetic(chunk, iat_end as u32),
    );
    linker
        .defined
        .insert("__ryn_import_directory".into(), Target::Synthetic(chunk, 0));
    // Symbols: __imp_NAME -> IAT slot; NAME -> thunk.
    let mut thunk = Vec::new();
    let mut thunk_fixups = Vec::new();
    let mut thunk_symbols = Vec::new();
    for (d, (_, names)) in dlls.iter().enumerate() {
        for (n, (name, _)) in names.iter().enumerate() {
            let slot = (iat_offsets[d] + n * 8) as u32;
            linker
                .defined
                .insert(format!("__imp_{name}"), Target::Synthetic(chunk, slot));
            linker.undefined.remove(&format!("__imp_{name}"));
            if !linker.defined.contains_key(name) {
                // jmp qword ptr [rip + __imp_name]
                let at = thunk.len() as u32;
                thunk.extend_from_slice(&[0xFF, 0x25, 0, 0, 0, 0, 0xCC, 0xCC]);
                thunk_fixups.push((at + 2, REL_REL32, format!("__imp_{name}")));
                thunk_symbols.push((name.clone(), at));
            }
        }
    }
    if !thunk.is_empty() {
        let thunk_chunk = linker.chunks.len();
        linker.chunks.push(Chunk {
            kind: OutKind::Text,
            data: thunk,
            align: 16,
            fixups: thunk_fixups,
            offset: 0,
            placed: false,
        });
        for (name, at) in thunk_symbols {
            linker
                .defined
                .insert(name.clone(), Target::Synthetic(thunk_chunk, at));
            linker.undefined.remove(&name);
        }
    }
}

struct OutSection {
    kind: OutKind,
    data: Vec<u8>,
    size: u32,
    rva: u32,
}

fn write_image(
    linker: &mut Linker,
    entry: &str,
    has_tls: bool,
    output: &Path,
) -> Result<(), LinkError> {
    // 1. Order input sections: by output kind, then by full name so `$` groups sort.
    let mut pieces: Vec<(OutKind, String, usize, usize)> = Vec::new();
    for (o, object) in linker.objects.iter().enumerate() {
        for (s, section) in object.sections.iter().enumerate() {
            if !linker.live[o][s] {
                continue;
            }
            if let Some(kind) = output_kind(section) {
                pieces.push((kind, section.name.clone(), o, s));
            }
        }
    }
    pieces.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    // TLS callbacks (.CRT$XL*) must be one null-terminated pointer array.
    let mut tls_callback_chunk = None;
    if has_tls {
        let chunk = linker.chunks.len();
        linker.chunks.push(Chunk {
            kind: OutKind::Rdata,
            data: vec![0; 8],
            align: 8,
            fixups: Vec::new(),
            offset: 0,
            placed: false,
        });
        tls_callback_chunk = Some(chunk);
    }

    let mut placed: HashMap<(usize, usize), Placed> = HashMap::new();
    let mut sizes: BTreeMap<OutKind, u32> = BTreeMap::new();
    let mut tls_callbacks_start: Option<(OutKind, u32)> = None;
    let kinds = [
        OutKind::Text,
        OutKind::Rdata,
        OutKind::Data,
        OutKind::Pdata,
        OutKind::Tls,
        OutKind::Bss,
    ];
    for kind in kinds {
        let mut cursor = 0u32;
        let mut in_callbacks = false;
        for (piece_kind, name, o, s) in pieces.iter().filter(|p| p.0 == kind) {
            let _ = piece_kind;
            let section = &linker.objects[*o].sections[*s];
            let is_callback = name.starts_with(".CRT$XL");
            if is_callback && !in_callbacks {
                cursor = align_up(cursor, 8);
                tls_callbacks_start = Some((kind, cursor));
                in_callbacks = true;
            } else if !is_callback && in_callbacks {
                // Terminate the callback array right after the last entry.
                if let Some(chunk) = tls_callback_chunk.take() {
                    linker.chunks[chunk].offset = cursor;
                    linker.chunks[chunk].placed = true;
                    cursor += 8;
                }
                in_callbacks = false;
            }
            if !is_callback {
                cursor = align_up(cursor, section.align.max(1));
            }
            placed.insert(
                (*o, *s),
                Placed {
                    kind,
                    offset: cursor,
                },
            );
            cursor += section.size;
        }
        if in_callbacks && let Some(chunk) = tls_callback_chunk.take() {
            linker.chunks[chunk].offset = cursor;
            linker.chunks[chunk].placed = true;
            cursor += 8;
        }
        for chunk in linker
            .chunks
            .iter_mut()
            .filter(|c| c.kind == kind && !c.placed)
        {
            cursor = align_up(cursor, chunk.align);
            chunk.offset = cursor;
            chunk.placed = true;
            cursor += chunk.data.len() as u32;
        }
        sizes.insert(kind, cursor);
    }
    // Without .CRT$XL contributions the terminator chunk is an empty callback array.
    if let Some(chunk) = tls_callback_chunk {
        tls_callbacks_start = Some((OutKind::Rdata, linker.chunks[chunk].offset));
    }
    if has_tls {
        let (kind, offset) = tls_callbacks_start.unwrap_or((OutKind::Rdata, 0));
        linker
            .defined
            .insert("__ryn_tls_callbacks".into(), Target::Output(kind, offset));
        linker
            .defined
            .insert("__ryn_tls_start".into(), Target::Output(OutKind::Tls, 0));
        linker.defined.insert(
            "__ryn_tls_end".into(),
            Target::Output(OutKind::Tls, *sizes.get(&OutKind::Tls).unwrap_or(&0)),
        );
    }

    // 2. Assign RVAs to output sections.
    let header_size = align_up(0x400, FILE_ALIGN);
    let mut rva = align_up(header_size, SECTION_ALIGN);
    let mut out: Vec<OutSection> = Vec::new();
    let mut section_rva: HashMap<OutKind, u32> = HashMap::new();
    for kind in kinds {
        let size = *sizes.get(&kind).unwrap_or(&0);
        if size == 0 {
            continue;
        }
        section_rva.insert(kind, rva);
        out.push(OutSection {
            kind,
            data: if kind == OutKind::Bss {
                Vec::new()
            } else {
                vec![0; size as usize]
            },
            size,
            rva,
        });
        rva = align_up(rva + size, SECTION_ALIGN);
    }

    let symbol_rva = |linker: &Linker, target: Target| -> Option<u32> {
        match target {
            Target::Section(o, s, value) => placed
                .get(&(o, s))
                .map(|p| section_rva[&p.kind] + p.offset + value),
            Target::Synthetic(chunk, value) => {
                let c = &linker.chunks[chunk];
                section_rva.get(&c.kind).map(|base| base + c.offset + value)
            }
            Target::Absolute(value) => Some(value),
            Target::Output(kind, offset) => section_rva.get(&kind).map(|base| base + offset),
            Target::ImageBase => Some(0),
        }
    };

    // 3. Copy section bytes and apply relocations.
    let mut base_relocations: Vec<u32> = Vec::new();
    let mut apply = |out: &mut Vec<OutSection>,
                     kind: OutKind,
                     at: u32,
                     rel: u16,
                     target_rva: u32,
                     target_section: Option<(u16, u32)>,
                     absolute: bool|
     -> Result<(), LinkError> {
        let section_index = out.iter().position(|s| s.kind == kind).unwrap();
        let place_rva = out[section_index].rva + at;
        let bytes = &mut out[section_index].data;
        let at = at as usize;
        let read32 = |b: &[u8]| i32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as i64;
        match rel {
            REL_ABSOLUTE => {}
            REL_ADDR64 => {
                let addend = i64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
                let value = if absolute {
                    target_rva as i64 + addend
                } else {
                    (IMAGE_BASE as i64) + target_rva as i64 + addend
                };
                bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
                if !absolute {
                    base_relocations.push(place_rva);
                }
            }
            REL_ADDR32 => {
                let value = IMAGE_BASE as i64 + target_rva as i64 + read32(bytes);
                if value > u32::MAX as i64 {
                    return err("ADDR32 relocation out of range; the image base is above 4 GB");
                }
                bytes[at..at + 4].copy_from_slice(&(value as u32).to_le_bytes());
            }
            REL_ADDR32NB => {
                let value = target_rva as i64 + read32(bytes);
                bytes[at..at + 4].copy_from_slice(&(value as u32).to_le_bytes());
            }
            REL_REL32..=REL_REL32_5 => {
                let extra = (rel - REL_REL32) as i64;
                let value = target_rva as i64 + read32(bytes) - (place_rva as i64 + 4 + extra);
                if value < i32::MIN as i64 || value > i32::MAX as i64 {
                    return err("REL32 relocation out of range");
                }
                bytes[at..at + 4].copy_from_slice(&(value as i32).to_le_bytes());
            }
            REL_SECTION => {
                let index = target_section.map(|(i, _)| i).unwrap_or(0);
                bytes[at..at + 2].copy_from_slice(&index.to_le_bytes());
            }
            REL_SECREL => {
                let base = target_section.map(|(_, base)| base).unwrap_or(0);
                let value = target_rva as i64 - base as i64 + read32(bytes);
                bytes[at..at + 4].copy_from_slice(&(value as u32).to_le_bytes());
            }
            other => return err(format!("unsupported relocation type {other}")),
        }
        Ok(())
    };

    let section_of_rva = |out: &Vec<OutSection>, rva: u32| -> Option<(u16, u32)> {
        out.iter()
            .enumerate()
            .find(|(_, s)| rva >= s.rva && rva < s.rva + s.size.max(1))
            .map(|(i, s)| ((i + 1) as u16, s.rva))
    };

    for (o, object) in linker.objects.iter().enumerate() {
        for (s, section) in object.sections.iter().enumerate() {
            let Some(place) = placed.get(&(o, s)) else {
                continue;
            };
            if place.kind != OutKind::Bss {
                let target = out.iter_mut().find(|x| x.kind == place.kind).unwrap();
                let start = place.offset as usize;
                target.data[start..start + section.data.len()].copy_from_slice(&section.data);
            }
            for relocation in &section.relocations {
                let symbol = object
                    .symbols
                    .get(relocation.symbol as usize)
                    .cloned()
                    .flatten()
                    .ok_or_else(|| LinkError(format!("{}: bad relocation symbol", object.name)))?;
                let (target_rva, absolute) = if symbol.section > 0
                    && symbol.class != SYM_EXTERNAL
                    && symbol.class != SYM_WEAK_EXTERNAL
                {
                    let ts = (symbol.section - 1) as usize;
                    match placed.get(&(o, ts)) {
                        Some(p) => (section_rva[&p.kind] + p.offset + symbol.value, false),
                        // A reference into a discarded section (debug info) is ignored.
                        None => continue,
                    }
                } else if symbol.section == -1 && symbol.class != SYM_EXTERNAL {
                    (symbol.value, true)
                } else {
                    let target = linker.defined.get(&symbol.name).copied().ok_or_else(|| {
                        LinkError(format!(
                            "undefined symbol `{}` referenced from {}",
                            symbol.name, object.name
                        ))
                    })?;
                    let absolute = matches!(target, Target::Absolute(_));
                    match symbol_rva(linker, target) {
                        Some(rva) => (rva, absolute),
                        None => continue,
                    }
                };
                let _ = (SYM_LABEL,);
                let target_section = section_of_rva(&out, target_rva);
                apply(
                    &mut out,
                    place.kind,
                    place.offset + relocation.offset,
                    relocation.kind,
                    target_rva,
                    target_section,
                    absolute,
                )?;
            }
        }
    }
    for c in 0..linker.chunks.len() {
        let (kind, offset, data, fixups) = {
            let chunk = &linker.chunks[c];
            (
                chunk.kind,
                chunk.offset,
                chunk.data.clone(),
                chunk.fixups.clone(),
            )
        };
        if kind != OutKind::Bss {
            let target = out.iter_mut().find(|x| x.kind == kind).unwrap();
            target.data[offset as usize..offset as usize + data.len()].copy_from_slice(&data);
        }
        for (at, rel, name) in fixups {
            let target = linker
                .defined
                .get(&name)
                .copied()
                .ok_or_else(|| LinkError(format!("undefined synthetic symbol `{name}`")))?;
            let target_rva = symbol_rva(linker, target).unwrap_or(0);
            // Import-table fixups store offsets relative to the chunk itself.
            let (target_rva, rel) = if rel == REL_ADDR32NB {
                (
                    target_rva
                        - match target {
                            Target::Synthetic(_, v) => v,
                            _ => 0,
                        },
                    rel,
                )
            } else {
                (target_rva, rel)
            };
            apply(&mut out, kind, offset + at, rel, target_rva, None, false)?;
        }
    }

    // 4. Data directories.
    let mut directories = [(0u32, 0u32); 16];
    let rva_of = |linker: &Linker, name: &str| {
        linker
            .defined
            .get(name)
            .and_then(|t| symbol_rva(linker, *t))
    };
    if let Some(import) = rva_of(linker, "__ryn_import_directory") {
        let count = linker.imports.len() as u32;
        directories[1] = (import, 20 * (count + 1));
        let start = rva_of(linker, "__ryn_iat_start").unwrap_or(0);
        let end = rva_of(linker, "__ryn_iat_end").unwrap_or(start);
        directories[12] = (start, end - start);
    }
    if let Some(pdata) = out.iter().find(|s| s.kind == OutKind::Pdata) {
        directories[3] = (pdata.rva, pdata.size);
    }
    if has_tls && let Some(directory) = rva_of(linker, "__ryn_tls_directory") {
        directories[9] = (directory, 40);
    }

    // 5. Base relocations, grouped per 4 KB page.
    base_relocations.sort_unstable();
    base_relocations.dedup();
    let mut reloc = Vec::new();
    let mut i = 0;
    while i < base_relocations.len() {
        let page = base_relocations[i] & !0xFFF;
        let start = reloc.len();
        reloc.extend_from_slice(&page.to_le_bytes());
        reloc.extend_from_slice(&[0; 4]);
        let mut count = 0;
        while i < base_relocations.len() && base_relocations[i] & !0xFFF == page {
            let entry: u16 = (10 << 12) | (base_relocations[i] & 0xFFF) as u16;
            reloc.extend_from_slice(&entry.to_le_bytes());
            count += 1;
            i += 1;
        }
        if count % 2 == 1 {
            reloc.extend_from_slice(&[0, 0]);
        }
        let block = (reloc.len() - start) as u32;
        reloc[start + 4..start + 8].copy_from_slice(&block.to_le_bytes());
    }

    // Recompute RVAs after the TLS directory may have grown .rdata.
    let mut next = align_up(header_size, SECTION_ALIGN);
    for section in &out {
        next = align_up(section.rva + section.size, SECTION_ALIGN);
    }
    let reloc_rva = next;
    if !reloc.is_empty() {
        directories[5] = (reloc_rva, reloc.len() as u32);
    }

    let entry_rva = rva_of(linker, entry)
        .ok_or_else(|| LinkError(format!("entry point `{entry}` is not defined")))?;
    type PreparedSection = ([u8; 8], u32, u32, Vec<u8>, u32);
    let mut sections: Vec<PreparedSection> = out
        .iter()
        .map(|s| {
            (
                *s.kind.name(),
                s.rva,
                s.size,
                s.data.clone(),
                s.kind.characteristics(),
            )
        })
        .collect();
    if !reloc.is_empty() {
        sections.push((
            *b".reloc\0\0",
            reloc_rva,
            reloc.len() as u32,
            reloc,
            SCN_CNT_INITIALIZED_DATA | SCN_MEM_READ | SCN_MEM_DISCARDABLE,
        ));
    }
    let image_size = sections
        .iter()
        .map(|s| align_up(s.1 + s.2, SECTION_ALIGN))
        .max()
        .unwrap_or(SECTION_ALIGN);

    // 6. Headers.
    let mut file = vec![0u8; header_size as usize];
    file[0..2].copy_from_slice(b"MZ");
    file[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    let pe = 0x80;
    file[pe..pe + 4].copy_from_slice(b"PE\0\0");
    let coff = pe + 4;
    file[coff..coff + 2].copy_from_slice(&MACHINE_AMD64.to_le_bytes());
    file[coff + 2..coff + 4].copy_from_slice(&(sections.len() as u16).to_le_bytes());
    file[coff + 16..coff + 18].copy_from_slice(&240u16.to_le_bytes());
    // EXECUTABLE_IMAGE | LARGE_ADDRESS_AWARE
    file[coff + 18..coff + 20].copy_from_slice(&0x0022u16.to_le_bytes());
    let opt = coff + 20;
    let code_size = sections
        .iter()
        .filter(|s| s.4 & SCN_CNT_CODE != 0)
        .map(|s| align_up(s.2, FILE_ALIGN))
        .sum::<u32>();
    let init_size = sections
        .iter()
        .filter(|s| s.4 & SCN_CNT_INITIALIZED_DATA != 0)
        .map(|s| align_up(s.2, FILE_ALIGN))
        .sum::<u32>();
    let uninit_size = sections
        .iter()
        .filter(|s| s.4 & SCN_CNT_UNINITIALIZED_DATA != 0)
        .map(|s| align_up(s.2, FILE_ALIGN))
        .sum::<u32>();
    let mut w = |offset: usize, bytes: &[u8]| {
        file[opt + offset..opt + offset + bytes.len()].copy_from_slice(bytes)
    };
    w(0, &0x20Bu16.to_le_bytes());
    w(2, &[14, 0]);
    w(4, &code_size.to_le_bytes());
    w(8, &init_size.to_le_bytes());
    w(12, &uninit_size.to_le_bytes());
    w(16, &entry_rva.to_le_bytes());
    w(
        20,
        &sections
            .iter()
            .find(|s| s.4 & SCN_CNT_CODE != 0)
            .map(|s| s.1)
            .unwrap_or(0)
            .to_le_bytes(),
    );
    w(24, &IMAGE_BASE.to_le_bytes());
    w(32, &SECTION_ALIGN.to_le_bytes());
    w(36, &FILE_ALIGN.to_le_bytes());
    w(40, &6u16.to_le_bytes()); // OS version 6.0
    w(48, &6u16.to_le_bytes()); // subsystem version 6.0
    w(56, &image_size.to_le_bytes());
    w(60, &header_size.to_le_bytes());
    w(68, &3u16.to_le_bytes()); // console subsystem
    // HIGH_ENTROPY_VA | DYNAMIC_BASE | NX_COMPAT | TERMINAL_SERVER_AWARE
    w(70, &0x8160u16.to_le_bytes());
    w(72, &(8u64 << 20).to_le_bytes()); // stack reserve 8 MB
    w(80, &0x1000u64.to_le_bytes()); // stack commit
    w(88, &(1u64 << 20).to_le_bytes()); // heap reserve
    w(96, &0x1000u64.to_le_bytes()); // heap commit
    w(108, &16u32.to_le_bytes());
    for (i, (rva, size)) in directories.iter().enumerate() {
        w(112 + i * 8, &rva.to_le_bytes());
        w(116 + i * 8, &size.to_le_bytes());
    }
    let section_table = opt + 240;
    let mut raw = header_size;
    let mut body = Vec::new();
    for (i, (name, rva, size, data, characteristics)) in sections.iter().enumerate() {
        let h = section_table + i * 40;
        if h + 40 > file.len() {
            return err("too many output sections for the header");
        }
        file[h..h + 8].copy_from_slice(name);
        file[h + 8..h + 12].copy_from_slice(&size.to_le_bytes());
        file[h + 12..h + 16].copy_from_slice(&rva.to_le_bytes());
        let raw_size = if characteristics & SCN_CNT_UNINITIALIZED_DATA != 0 {
            0
        } else {
            align_up(data.len() as u32, FILE_ALIGN)
        };
        file[h + 16..h + 20].copy_from_slice(&raw_size.to_le_bytes());
        file[h + 20..h + 24].copy_from_slice(&(if raw_size == 0 { 0 } else { raw }).to_le_bytes());
        file[h + 36..h + 40].copy_from_slice(&characteristics.to_le_bytes());
        if raw_size > 0 {
            body.extend_from_slice(data);
            body.resize(body.len() + (raw_size as usize - data.len()), 0);
            raw += raw_size;
        }
    }
    file.extend_from_slice(&body);
    fs::write(output, &file)
        .map_err(|error| LinkError(format!("cannot write {}: {error}", output.display())))
}

/// A readable list of what the archive would pull in for these objects; used
/// when a link fails so the report names the missing symbols.
pub fn describe_undefined(names: &[String]) -> String {
    let mut out = String::new();
    for name in names {
        let _ = writeln!(out, "  {name}");
    }
    out
}
