//! Signed manifest for a packaged, self-contained Scarlet PIE.

use super::PAGE;
use crate::in_process::Patch;
use scarlet_abi::tls::aarch64_elf_tls_offset;
use std::{collections::BTreeSet, io, ops::Range};

const MAX_SPAN: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Access {
    ReadOnly,
    ReadWrite,
    Execute,
}

pub(super) struct Segment {
    pub access: Access,
    pub address: usize,
    pub size: usize,
    pub source: Range<usize>,
}

impl Segment {
    pub fn pages(&self) -> Range<usize> {
        align_down(self.address)..align_up(self.address + self.size)
    }

    fn contains(&self, address: usize, length: usize) -> bool {
        address >= self.address
            && address
                .checked_add(length)
                .is_some_and(|end| end <= self.address + self.size)
    }
}

pub(super) struct Tls {
    pub template: Range<usize>,
    pub offset: usize,
    pub alignment: usize,
    pub memory_size: usize,
}

pub(super) struct Manifest {
    pub hash: String,
    pub span: usize,
    pub entry: usize,
    pub startup: Option<usize>,
    pub phdr: Option<(usize, usize)>,
    pub segments: Vec<Segment>,
    pub relro: Vec<Range<usize>>,
    pub dependencies: Vec<(String, String)>,
    pub tls: Option<Tls>,
    pub relocs: Vec<(usize, usize)>,
    pub patches: Vec<(usize, Patch)>,
}

pub(super) fn align_up(value: usize) -> usize {
    value.next_multiple_of(PAGE)
}

fn align_down(value: usize) -> usize {
    value & !(PAGE - 1)
}

fn invalid(error: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn number(value: &str) -> io::Result<usize> {
    value
        .parse()
        .map_err(|_| invalid("Invalid image manifest number"))
}

fn range(start: usize, len: usize) -> io::Result<Range<usize>> {
    Ok(start
        ..start
            .checked_add(len)
            .ok_or_else(|| invalid("Image manifest range overflow"))?)
}

impl Manifest {
    pub fn parse(manifest: &str, data_size: usize, rx_size: usize) -> io::Result<Self> {
        let mut lines = manifest.lines();
        if !matches!(
            lines.next(),
            Some("scarlet-ios-image 1" | "scarlet-ios-image 2")
        ) || lines.next() != Some("abi scarlet")
        {
            return Err(invalid("Unsupported signed image version or ABI"));
        }
        let mut header = |key| {
            lines
                .next()
                .and_then(|line| line.strip_prefix(key))
                .ok_or_else(|| invalid("Missing image manifest header field"))
        };
        let hash = header("sha256 ")?;
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid("Invalid ELF image digest"));
        }
        let span = number(header("span ")?)?;
        let entry = number(header("entry ")?)?;
        if span == 0 || span > MAX_SPAN || !span.is_multiple_of(PAGE) || !entry.is_multiple_of(4) {
            return Err(invalid("Invalid image span or entry alignment"));
        }
        let mut result = Self {
            hash: hash.into(),
            span,
            entry,
            startup: None,
            phdr: None,
            segments: Vec::new(),
            relro: Vec::new(),
            dependencies: Vec::new(),
            tls: None,
            relocs: Vec::new(),
            patches: Vec::new(),
        };
        let mut targets = BTreeSet::new();
        let mut patch_sites = BTreeSet::new();
        for line in lines {
            let items: Vec<_> = line.split_ascii_whitespace().collect();
            match items.as_slice() {
                ["startup", pc] if result.startup.is_none() => {
                    result.startup = Some(number(pc)?);
                }
                ["phdr", address, count] if result.phdr.is_none() => {
                    let (address, count) = (number(address)?, number(count)?);
                    if count == 0 || count >= u16::MAX as usize {
                        return Err(invalid("Invalid ELF program header count"));
                    }
                    result.phdr = Some((address, count));
                }
                ["segment", access, address, filed, size, source] => {
                    let access = match *access {
                        "ro" => Access::ReadOnly,
                        "rw" => Access::ReadWrite,
                        "rx" => Access::Execute,
                        _ => return Err(invalid("Invalid signed segment permissions")),
                    };
                    let address = number(address)?;
                    let size = number(size)?;
                    let source = range(number(source)?, number(filed)?)?;
                    let end = range(address, size)?.end;
                    if size == 0
                        || end > span
                        || source.len() > size
                        || (access != Access::Execute && source.end > data_size)
                        || result
                            .segments
                            .last()
                            .is_some_and(|s| s.pages().end > align_down(address))
                    {
                        return Err(invalid("Overlapping or out of bounds signed image segment"));
                    }
                    result.segments.push(Segment {
                        access,
                        address,
                        size,
                        source,
                    });
                }
                ["relro", begin, end] => {
                    let (begin, end) = (number(begin)?, number(end)?);
                    if begin >= end || end > span {
                        return Err(invalid("Invalid RELRO span"));
                    }
                    result.relro.push(align_down(begin)..align_up(end));
                }
                ["tls", start, filed, size, align] if result.tls.is_none() => {
                    let (start, filed, size) = (number(start)?, number(filed)?, number(size)?);
                    let template = range(start, filed)?;
                    let alignment = number(align)?;
                    let offset = aarch64_elf_tls_offset(alignment, size)
                        .ok_or_else(|| invalid("Unsupported static TLS layout"))?;
                    if filed > size || template.end > span {
                        return Err(invalid("Invalid static TLS template size"));
                    }
                    result.tls = Some(Tls {
                        template,
                        offset,
                        alignment,
                        memory_size: size,
                    });
                }
                ["reloc", target, value] => {
                    let (target, value) = (number(target)?, number(value)?);
                    if !targets.insert(target) || !target.is_multiple_of(8) || value > span {
                        return Err(invalid("Invalid signed relocation"));
                    }
                    result.relocs.push((target, value));
                }
                ["needed", name, digest] => {
                    if name.is_empty()
                        || !name
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
                        || matches!(*name, "." | "..")
                        || digest.len() != 64
                        || !digest.bytes().all(|b| b.is_ascii_hexdigit())
                        || result.dependencies.iter().any(|(seen, _)| seen == name)
                    {
                        return Err(invalid("Invalid signed dependency"));
                    }
                    result.dependencies.push(((*name).into(), (*digest).into()));
                }
                ["patch", pc, original] => {
                    let pc = number(pc)?;
                    let word = u32::from_str_radix(original, 16)
                        .map_err(|_| invalid("Invalid original patch instruction"))?;
                    let patch = Patch::from_guest_word(word)
                        .ok_or_else(|| invalid("Unsupported instruction patch"))?;
                    if !pc.is_multiple_of(4) || !patch_sites.insert(pc) {
                        return Err(invalid("Invalid instruction patch position"));
                    }
                    result.patches.push((pc, patch));
                }
                _ => return Err(invalid("Unknown or duplicate image manifest record")),
            }
        }
        result.validate(rx_size)?;
        Ok(result)
    }

    fn validate(&self, rx_size: usize) -> io::Result<()> {
        let file_bounds = |address| {
            self.segments.iter().any(|rx| {
                rx.access == Access::Execute
                    && rx.contains(address, 4)
                    && address - rx.address + 4 <= rx.source.len()
            })
        };
        if self.rx_pages().map(|pages| pages.len()).sum::<usize>() != rx_size
            || !file_bounds(self.entry)
            || self
                .startup
                .is_some_and(|pc| !pc.is_multiple_of(4) || !file_bounds(pc))
            || self
                .patches
                .iter()
                .any(|&(address, _)| !file_bounds(address))
        {
            return Err(invalid("Invalid signed RX layout or entry"));
        }
        let covers = |range: Range<usize>| {
            self.segments.iter().any(|s| {
                s.access == Access::ReadWrite
                    && s.pages().start <= range.start
                    && range.end <= s.pages().end
            })
        };
        let file_contains = |address: usize, size: usize| {
            self.segments.iter().any(|segment| {
                segment.contains(address, size)
                    && (address - segment.address) + size <= segment.source.len()
            })
        };
        if self
            .phdr
            .is_some_and(|(address, count)| !file_contains(address, 56 * count))
            || self.relro.iter().any(|relro| !covers(relro.clone()))
            || self.relocs.iter().any(|&(target, value)| {
                !self
                    .segments
                    .iter()
                    .any(|s| s.access == Access::ReadWrite && s.contains(target, 8))
                    || !self.segments.iter().any(|s| s.contains(value, 0))
            })
            || self.tls.as_ref().is_some_and(|tls| {
                !self.segments.iter().any(|s| {
                    s.contains(tls.template.start, tls.template.len())
                        && tls.template.end - s.address <= s.source.len()
                })
            })
        {
            return Err(invalid("Invalid signed RELRO, TLS or relocation range"));
        }
        Ok(())
    }

    pub fn rx_pages(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        self.segments
            .iter()
            .filter(|s| s.access == Access::Execute)
            .map(Segment::pages)
    }

    pub fn non_executable_ranges(&self, end: usize) -> impl Iterator<Item = Range<usize>> + '_ {
        self.rx_pages()
            .chain(std::iter::once(end..end))
            .scan(0, |previous, rx| {
                let gap = *previous..rx.start;
                *previous = rx.end;
                Some(gap)
            })
            .filter(|gap| !gap.is_empty())
    }
}
