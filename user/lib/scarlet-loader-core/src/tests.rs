use super::*;
use std::collections::BTreeMap;

#[derive(Clone)]
struct Definition {
    name: &'static str,
    value: usize,
    defined: bool,
    binding: u8,
    visibility: u8,
    kind: u8,
}
impl Definition {
    fn export(name: &'static str, value: usize) -> Self {
        Self {
            name,
            value,
            defined: true,
            binding: 1,
            visibility: 0,
            kind: 2,
        }
    }
    fn import(name: &'static str) -> Self {
        Self {
            name,
            value: 0,
            defined: false,
            binding: 1,
            visibility: 0,
            kind: 2,
        }
    }
}
struct Fixture {
    machine: Machine,
    needed: Vec<&'static str>,
    symbols: Vec<Definition>,
    relocations: Vec<(usize, usize, u32, i64)>,
    tags: Vec<(u64, u64)>,
    gnu: bool,
    sysv: bool,
    relro: bool,
    init: bool,
}
impl Default for Fixture {
    fn default() -> Self {
        Self {
            machine: Machine::Aarch64,
            needed: vec![],
            symbols: vec![],
            relocations: vec![],
            tags: vec![],
            gnu: false,
            sysv: true,
            relro: false,
            init: false,
        }
    }
}
impl Fixture {
    fn bytes(&self) -> Vec<u8> {
        let mut bytes = vec![0; 0x1800];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        put16(&mut bytes, 16, 3);
        put16(
            &mut bytes,
            18,
            if self.machine == Machine::Aarch64 {
                183
            } else {
                243
            },
        );
        put32(&mut bytes, 20, 1);
        put64(&mut bytes, 24, 0x200);
        put64(&mut bytes, 32, 64);
        put16(&mut bytes, 52, 64);
        put16(&mut bytes, 54, 56);
        put16(&mut bytes, 56, if self.relro { 4 } else { 3 });
        ph(&mut bytes, 64, 1, 5, 0, 0, 0x800, 0x800, 0x1000);
        ph(&mut bytes, 120, 1, 6, 0x1000, 0x1000, 0x800, 0x1000, 0x1000);
        if self.relro {
            ph(
                &mut bytes, 232, 0x6474e552, 4, 0x1000, 0x1000, 0x800, 0x1000, 1,
            );
        }
        let mut strings = vec![0];
        let mut tags = vec![(5, 0x300), (6, 0x400), (11, 24)];
        for needed in &self.needed {
            let index = strings.len();
            strings.extend_from_slice(needed.as_bytes());
            strings.push(0);
            tags.push((1, index as u64));
        }
        for (i, symbol) in self.symbols.iter().enumerate() {
            let index = strings.len();
            strings.extend_from_slice(symbol.name.as_bytes());
            strings.push(0);
            let offset = 0x400 + (i + 1) * 24;
            put32(&mut bytes, offset, index as u32);
            bytes[offset + 4] = (symbol.binding << 4) | symbol.kind;
            bytes[offset + 5] = symbol.visibility;
            put16(&mut bytes, offset + 6, u16::from(symbol.defined));
            put64(&mut bytes, offset + 8, symbol.value as u64);
        }
        bytes[0x300..0x300 + strings.len()].copy_from_slice(&strings);
        tags.push((10, strings.len() as u64));
        let symbol_count = self.symbols.len() + 1;
        if self.sysv {
            tags.push((4, 0x500));
            put32(&mut bytes, 0x500, 1);
            put32(&mut bytes, 0x504, symbol_count as u32);
            put32(&mut bytes, 0x508, if symbol_count > 1 { 1 } else { 0 });
        }
        if self.gnu {
            tags.push((0x6fff_fef5, 0x580));
            put32(&mut bytes, 0x580, 1);
            put32(&mut bytes, 0x584, 1);
            put32(&mut bytes, 0x588, 1);
            put32(&mut bytes, 0x58c, 6);
            put32(&mut bytes, 0x598, if symbol_count > 1 { 1 } else { 0 });
            for i in 1..symbol_count {
                put32(
                    &mut bytes,
                    0x59c + (i - 1) * 4,
                    if i == symbol_count - 1 { 1 } else { 2 },
                );
            }
        }
        let mut relocations = self.relocations.clone();
        if self.init {
            tags.push((25, 0x1300));
            tags.push((27, 8));
            relocations.push((
                0x1300,
                0,
                if self.machine == Machine::Aarch64 {
                    1027
                } else {
                    3
                },
                0x210,
            ));
        }
        if !relocations.is_empty() {
            tags.extend([(7, 0x600), (8, (relocations.len() * 24) as u64), (9, 24)]);
            for (i, (target, symbol, kind, addend)) in relocations.iter().enumerate() {
                put64(&mut bytes, 0x600 + i * 24, *target as u64);
                put64(
                    &mut bytes,
                    0x608 + i * 24,
                    ((*symbol as u64) << 32) | (*kind as u64),
                );
                put64(&mut bytes, 0x610 + i * 24, *addend as u64);
            }
        }
        tags.extend_from_slice(&self.tags);
        tags.push((0, 0));
        ph(
            &mut bytes,
            176,
            2,
            6,
            0x1000,
            0x1000,
            (tags.len() * 16) as u64,
            (tags.len() * 16) as u64,
            8,
        );
        for (i, (tag, value)) in tags.iter().enumerate() {
            put64(&mut bytes, 0x1000 + i * 16, *tag);
            put64(&mut bytes, 0x1008 + i * 16, *value);
        }
        bytes
    }
}
fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
#[allow(clippy::too_many_arguments)]
fn ph(
    bytes: &mut [u8],
    offset: usize,
    kind: u32,
    flags: u32,
    file: u64,
    address: u64,
    size: u64,
    memory: u64,
    align: u64,
) {
    put32(bytes, offset, kind);
    put32(bytes, offset + 4, flags);
    put64(bytes, offset + 8, file);
    put64(bytes, offset + 16, address);
    put64(bytes, offset + 32, size);
    put64(bytes, offset + 40, memory);
    put64(bytes, offset + 48, align);
}
#[derive(Default)]
struct Fake {
    files: BTreeMap<String, Vec<u8>>,
    mappings: BTreeMap<usize, Vec<u8>>,
    protections: Vec<(usize, usize, Permissions)>,
    unmaps: usize,
    fail_protect: bool,
}
impl Fake {
    fn with(files: &[(&str, Vec<u8>)]) -> Self {
        Self {
            files: files
                .iter()
                .map(|(name, bytes)| (name.to_string(), bytes.clone()))
                .collect(),
            ..Self::default()
        }
    }
    fn word(&mut self, address: usize) -> u64 {
        let mut bytes = [0; 8];
        self.read(address, &mut bytes).unwrap();
        u64::from_le_bytes(bytes)
    }
}
impl Platform for Fake {
    fn read_file(&mut self, _: Option<&str>, name: &str) -> Result<File, Error> {
        Ok(File {
            identity: name.to_string(),
            bytes: self
                .files
                .get(name)
                .ok_or_else(|| Error::Platform(name.to_string()))?
                .clone(),
        })
    }
    fn map(&mut self, size: usize, align: usize) -> Result<usize, Error> {
        let next = self
            .mappings
            .last_key_value()
            .map_or(0x100000, |(base, bytes)| base + bytes.len());
        let base = (next + align - 1) & !(align - 1);
        self.mappings.insert(base, vec![0; size]);
        Ok(base)
    }
    fn write(&mut self, address: usize, bytes: &[u8]) -> Result<(), Error> {
        for (base, memory) in &mut self.mappings {
            if let Some(offset) = address.checked_sub(*base)
                && let Some(destination) = memory.get_mut(offset..offset + bytes.len())
            {
                destination.copy_from_slice(bytes);
                return Ok(());
            }
        }
        Err(Error::Platform("write outside map".to_string()))
    }
    fn read(&mut self, address: usize, bytes: &mut [u8]) -> Result<(), Error> {
        for (base, memory) in &self.mappings {
            if let Some(offset) = address.checked_sub(*base)
                && let Some(source) = memory.get(offset..offset + bytes.len())
            {
                bytes.copy_from_slice(source);
                return Ok(());
            }
        }
        Err(Error::Platform("read outside map".to_string()))
    }
    fn protect(
        &mut self,
        address: usize,
        size: usize,
        permissions: Permissions,
    ) -> Result<(), Error> {
        if self.fail_protect {
            return Err(Error::Platform("protect failed".to_string()));
        }
        self.protections.push((address, size, permissions));
        Ok(())
    }
    fn unmap(&mut self, address: usize, _: usize) {
        self.unmaps += 1;
        self.mappings.remove(&address);
    }
}

#[test]
fn relative_relocations_and_zero_filled_bss_on_both_architectures() {
    for machine in [Machine::Aarch64, Machine::Riscv64] {
        let fixture = Fixture {
            machine,
            relocations: vec![(
                0x1200,
                0,
                if machine == Machine::Aarch64 { 1027 } else { 3 },
                -16,
            )],
            ..Fixture::default()
        };
        let mut loader = LoaderContext::new(Fake::with(&[("main", fixture.bytes())]), machine);
        let main = loader.load("main").unwrap();
        assert_eq!(loader.entry(main).unwrap(), 0x100200);
        assert_eq!(loader.platform_mut().word(0x101200), 0x100000 - 16);
        assert_eq!(loader.platform_mut().word(0x101800), 0);
        assert!(
            loader
                .platform()
                .protections
                .iter()
                .all(|(_, _, p)| !(p.write && p.execute))
        );
    }
}
#[test]
fn dependency_interposition_is_breadth_first_not_depth_first() {
    let root = Fixture {
        needed: vec!["left", "right"],
        symbols: vec![Definition::import("answer")],
        relocations: vec![(0x1200, 1, 257, 7)],
        ..Fixture::default()
    };
    let left = Fixture {
        needed: vec!["leaf"],
        ..Fixture::default()
    };
    let right = Fixture {
        symbols: vec![Definition::export("answer", 0x220)],
        ..Fixture::default()
    };
    let leaf = Fixture {
        symbols: vec![Definition::export("answer", 0x230)],
        ..Fixture::default()
    };
    let mut loader = LoaderContext::new(
        Fake::with(&[
            ("main", root.bytes()),
            ("left", left.bytes()),
            ("right", right.bytes()),
            ("leaf", leaf.bytes()),
        ]),
        Machine::Aarch64,
    );
    let main = loader.load("main").unwrap();
    assert_eq!(loader.object_count(), 4);
    assert_eq!(loader.platform_mut().word(0x101200), 0x104227);
    assert_eq!(loader.lookup(main, "answer").unwrap(), Some(0x104220));
}
#[test]
fn undefined_weak_is_zero_and_runtime_exports_are_fallback() {
    let mut weak = Definition::import("optional");
    weak.binding = 2;
    let fixture = Fixture {
        symbols: vec![weak, Definition::import("dlopen")],
        relocations: vec![(0x1200, 1, 1025, 0), (0x1208, 2, 1026, 0)],
        ..Fixture::default()
    };
    let mut loader = LoaderContext::new(Fake::with(&[("main", fixture.bytes())]), Machine::Aarch64);
    loader.add_symbol("dlopen", 0xabc);
    let main = loader.load("main").unwrap();
    assert_eq!(loader.platform_mut().word(0x101200), 0);
    assert_eq!(loader.platform_mut().word(0x101208), 0xabc);
    assert_eq!(loader.lookup(main, "optional").unwrap(), None);
    assert_eq!(loader.lookup_global("dlopen").unwrap(), Some(0xabc));
}
#[test]
fn riscv_jump_slot_ignores_rela_addend() {
    let fixture = Fixture {
        machine: Machine::Riscv64,
        symbols: vec![Definition::import("host")],
        relocations: vec![(0x1200, 1, 5, 99)],
        ..Fixture::default()
    };
    let mut loader = LoaderContext::new(Fake::with(&[("main", fixture.bytes())]), Machine::Riscv64);
    loader.add_symbol("host", 0xabcd);
    loader.load("main").unwrap();
    assert_eq!(loader.platform_mut().word(0x101200), 0xabcd);
}
#[test]
fn protected_and_hidden_symbols_bind_locally() {
    for visibility in [2, 3] {
        let main = Fixture {
            needed: vec!["dep"],
            symbols: vec![Definition::export("same", 0x210)],
            ..Fixture::default()
        };
        let mut symbol = Definition::export("same", 0x220);
        symbol.visibility = visibility;
        let dep = Fixture {
            symbols: vec![symbol],
            relocations: vec![(0x1200, 1, 257, 0)],
            ..Fixture::default()
        };
        let mut loader = LoaderContext::new(
            Fake::with(&[("main", main.bytes()), ("dep", dep.bytes())]),
            Machine::Aarch64,
        );
        loader.load("main").unwrap();
        assert_eq!(loader.platform_mut().word(0x103200), 0x102220);
    }
}
#[test]
fn missing_symbol_rolls_back_all_new_mappings_but_keeps_old_scope() {
    let old = Fixture::default();
    let bad = Fixture {
        symbols: vec![Definition::import("missing")],
        relocations: vec![(0x1200, 1, 257, 0)],
        ..Fixture::default()
    };
    let mut loader = LoaderContext::new(
        Fake::with(&[("old", old.bytes()), ("bad", bad.bytes())]),
        Machine::Aarch64,
    );
    let old = loader.load("old").unwrap();
    assert_eq!(
        loader.load("bad"),
        Err(Error::MissingSymbol("missing".to_string()))
    );
    assert_eq!(loader.object_count(), 1);
    assert_eq!(loader.platform().mappings.len(), 1);
    assert_eq!(loader.platform().unmaps, 1);
    assert_eq!(loader.entry(old).unwrap(), 0x100200);
}
#[test]
fn final_protection_failure_rolls_back_mappings() {
    let mut platform = Fake::with(&[("main", Fixture::default().bytes())]);
    platform.fail_protect = true;
    let mut loader = LoaderContext::new(platform, Machine::Aarch64);
    assert!(loader.load("main").is_err());
    assert_eq!(loader.object_count(), 0);
    assert_eq!(loader.platform().mappings.len(), 0);
    assert_eq!(loader.platform().unmaps, 1);
}
#[test]
fn duplicate_and_cyclic_dependencies_map_once() {
    let main = Fixture {
        needed: vec!["dep", "dep"],
        ..Fixture::default()
    };
    let dep = Fixture {
        needed: vec!["main"],
        ..Fixture::default()
    };
    let mut loader = LoaderContext::new(
        Fake::with(&[("main", main.bytes()), ("dep", dep.bytes())]),
        Machine::Aarch64,
    );
    let first = loader.load("main").unwrap();
    assert_eq!(loader.load("main").unwrap(), first);
    assert_eq!(loader.object_count(), 2);
}
#[test]
fn constructors_run_dependency_first_once_and_relro_becomes_readonly() {
    let main = Fixture {
        needed: vec!["dep"],
        init: true,
        relro: true,
        ..Fixture::default()
    };
    let dep = Fixture {
        init: true,
        ..Fixture::default()
    };
    let mut loader = LoaderContext::new(
        Fake::with(&[("main", main.bytes()), ("dep", dep.bytes())]),
        Machine::Aarch64,
    );
    loader.load("main").unwrap();
    assert_eq!(
        loader.take_pending_initializers().unwrap(),
        vec![0x102210, 0x100210]
    );
    assert!(loader.take_pending_initializers().unwrap().is_empty());
    assert!(
        loader
            .platform()
            .protections
            .iter()
            .any(|(a, _, p)| *a == 0x101000 && p.read && !p.write)
    );
}
#[test]
fn adopted_main_is_never_copied_unmapped_or_auto_initialized() {
    let main = Fixture {
        init: true,
        needed: vec!["dep"],
        ..Fixture::default()
    };
    let dep = Fixture {
        init: true,
        ..Fixture::default()
    };
    let mut platform = Fake::with(&[("main", main.bytes()), ("dep", dep.bytes())]);
    // Kernel mapping already contains image bytes and distinguishable BSS data.
    let mut image = vec![0; 0x2000];
    image[..0x1800].copy_from_slice(&main.bytes());
    image[0x1900] = 77;
    platform.mappings.insert(0x200000, image);
    let mut loader = LoaderContext::new(platform, Machine::Aarch64);
    let id = unsafe { loader.load_existing("main", 0x200000) }.unwrap();
    assert_eq!(loader.entry(id).unwrap(), 0x200200);
    assert_eq!(loader.platform().mappings[&0x200000][0x1900], 77);
    assert_eq!(loader.take_pending_initializers().unwrap(), vec![0x202210]);
    assert_eq!(loader.constructors(id).unwrap(), vec![0x200210]);
    assert_eq!(loader.objects[0].owned_mapping, None);
}
#[test]
fn adopted_main_unchanged_when_any_symbol_is_unresolved() {
    let fixture = Fixture {
        symbols: vec![Definition::import("missing")],
        relocations: vec![(0x1200, 0, 1027, 0x200), (0x1208, 1, 257, 0)],
        ..Fixture::default()
    };
    let mut platform = Fake::with(&[("main", fixture.bytes())]);
    platform.mappings.insert(0x200000, vec![42; 0x2000]);
    let mut loader = LoaderContext::new(platform, Machine::Aarch64);
    assert!(unsafe { loader.load_existing("main", 0x200000) }.is_err());
    assert_eq!(loader.platform().mappings[&0x200000], vec![42; 0x2000]);
    assert_eq!(loader.platform().unmaps, 0);
}
#[test]
fn supports_gnu_and_sysv_hashes_independently() {
    for (gnu, sysv) in [(true, false), (false, true), (true, true)] {
        let fixture = Fixture {
            gnu,
            sysv,
            symbols: vec![Definition::export("answer", 0x210)],
            ..Fixture::default()
        };
        let mut loader =
            LoaderContext::new(Fake::with(&[("main", fixture.bytes())]), Machine::Aarch64);
        let id = loader.load("main").unwrap();
        assert_eq!(loader.lookup(id, "answer").unwrap(), Some(0x100210));
    }
}
#[test]
fn malformed_hashes_fail_with_checked_bounds() {
    let fixture = Fixture {
        gnu: true,
        sysv: false,
        symbols: vec![Definition::export("x", 0x210)],
        ..Fixture::default()
    };
    let mut bad = fixture.bytes();
    put32(&mut bad, 0x598, u32::MAX);
    assert!(Parsed::parse(&bad, Machine::Aarch64).is_err());
    let mut bad = fixture.bytes();
    put32(&mut bad, 0x59c, 2);
    assert!(Parsed::parse(&bad, Machine::Aarch64).is_err());
    let mut bad = Fixture::default().bytes();
    put32(&mut bad, 0x508, 99);
    assert!(Parsed::parse(&bad, Machine::Aarch64).is_err());
}
#[test]
fn unsupported_dynamic_features_are_rejected_before_mapping() {
    for tag in [
        16,
        22,
        35,
        36,
        37,
        0x6fff_fff0,
        0x6fff_fffc,
        0x6fff_fffe,
        15,
        29,
    ] {
        let fixture = Fixture {
            tags: vec![(tag, 1)],
            ..Fixture::default()
        };
        let mut loader =
            LoaderContext::new(Fake::with(&[("main", fixture.bytes())]), Machine::Aarch64);
        assert!(matches!(loader.load("main"), Err(Error::Unsupported(_))));
        assert!(loader.platform().mappings.is_empty());
    }
}
#[test]
fn tls_ifunc_unknown_relocations_and_bad_targets_fail() {
    let mut tls = Fixture::default().bytes();
    put32(&mut tls, 176, 7);
    assert!(matches!(
        Parsed::parse(&tls, Machine::Aarch64),
        Err(Error::Unsupported("PT_TLS"))
    ));
    for kind in [6, 10] {
        let mut symbol = Definition::export("x", 0x210);
        symbol.kind = kind;
        assert!(
            Parsed::parse(
                &Fixture {
                    symbols: vec![symbol],
                    ..Fixture::default()
                }
                .bytes(),
                Machine::Aarch64
            )
            .is_err()
        );
    }
    for (target, kind, symbol) in [
        (0x200, 1027, 0),
        (0x1fff, 1027, 0),
        (0x1200, 999, 0),
        (0x1200, 1027, 1),
    ] {
        assert!(
            Parsed::parse(
                &Fixture {
                    relocations: vec![(target, symbol, kind, 0)],
                    ..Fixture::default()
                }
                .bytes(),
                Machine::Aarch64
            )
            .is_err()
        );
    }
}
#[test]
fn truncated_files_and_overflowing_ranges_never_panic() {
    let bytes = Fixture::default().bytes();
    for size in 0..bytes.len() {
        assert!(Parsed::parse(&bytes[..size], Machine::Aarch64).is_err());
    }
    for offset in [32, 64 + 8, 64 + 16, 64 + 32, 64 + 40, 120 + 16, 176 + 32] {
        let mut bad = bytes.clone();
        put64(&mut bad, offset, u64::MAX);
        assert!(Parsed::parse(&bad, Machine::Aarch64).is_err());
    }
}
#[test]
fn executable_image_can_only_be_adopted_without_bias() {
    let mut bytes = Fixture::default().bytes();
    put16(&mut bytes, 16, 2);
    let mut loader = LoaderContext::new(Fake::with(&[("main", bytes)]), Machine::Aarch64);
    assert!(matches!(loader.load("main"), Err(Error::Unsupported(_))));
    assert!(unsafe { loader.load_existing("main", 0x200000) }.is_err());
    assert!(unsafe { loader.load_existing("main", 0) }.is_ok());
}
#[test]
fn handle_lookup_does_not_leak_unrelated_object_symbols() {
    let main = Fixture {
        symbols: vec![Definition::export("main_symbol", 0x210)],
        ..Fixture::default()
    };
    let plugin = Fixture::default();
    let mut loader = LoaderContext::new(
        Fake::with(&[("main", main.bytes()), ("plugin", plugin.bytes())]),
        Machine::Aarch64,
    );
    loader.load("main").unwrap();
    let plugin = loader.load("plugin").unwrap();
    assert_eq!(loader.lookup(plugin, "main_symbol").unwrap(), None);
    assert_eq!(loader.lookup_global("main_symbol").unwrap(), Some(0x100210));
}

#[test]
fn malformed_constructor_is_rejected_transactionally() {
    let fixture = Fixture {
        tags: vec![(25, 0x1300), (27, 8)],
        relocations: vec![(0x1300, 0, 1027, 0x1200)],
        ..Fixture::default()
    };
    let mut loader = LoaderContext::new(Fake::with(&[("main", fixture.bytes())]), Machine::Aarch64);
    assert_eq!(
        loader.load("main"),
        Err(Error::Format(
            "constructor is outside executable loaded segments"
        ))
    );
    assert_eq!(loader.object_count(), 0);
    assert!(loader.platform().mappings.is_empty());
}
#[test]
fn oversized_dynamic_table_rejected_before_entry_iteration() {
    let mut bytes = Fixture::default().bytes();
    let size = 4097 * 16;
    bytes.resize(0x1000 + size, 0);
    // The first entry is DT_NULL. Even this terminated table must fail the size
    // cap before parsing; all containing file and virtual ranges are valid.
    bytes[0x1000..].fill(0);
    put64(&mut bytes, 120 + 32, size as u64);
    put64(&mut bytes, 120 + 40, size as u64);
    put64(&mut bytes, 176 + 32, size as u64);
    put64(&mut bytes, 176 + 40, size as u64);
    assert!(matches!(
        Parsed::parse(&bytes, Machine::Aarch64),
        Err(Error::Unsupported("dynamic table exceeds entry limit"))
    ));
}
#[test]
fn excessive_dt_needed_entries_are_bounded_before_name_allocation() {
    let mut bytes = Fixture::default().bytes();
    let size = 258 * 16;
    bytes.resize(0x1000 + size, 0);
    put64(&mut bytes, 120 + 32, size as u64);
    put64(&mut bytes, 120 + 40, size as u64);
    put64(&mut bytes, 176 + 32, size as u64);
    put64(&mut bytes, 176 + 40, size as u64);
    for i in 0..257 {
        put64(&mut bytes, 0x1000 + i * 16, 1);
        put64(&mut bytes, 0x1008 + i * 16, 0);
    }
    assert!(matches!(
        Parsed::parse(&bytes, Machine::Aarch64),
        Err(Error::Unsupported("too many DT_NEEDED entries"))
    ));
}
#[test]
fn partial_trailing_relro_page_preserves_writable_data() {
    let mut bytes = Fixture {
        relro: true,
        ..Fixture::default()
    }
    .bytes();
    put64(&mut bytes, 232 + 40, 0x800);
    let mut loader = LoaderContext::new(Fake::with(&[("main", bytes)]), Machine::Aarch64);
    loader.load("main").unwrap();
    assert!(
        loader
            .platform()
            .protections
            .iter()
            .any(|(a, _, p)| *a == 0x101000 && p.write)
    );
}
