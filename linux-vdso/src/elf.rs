//! Symbol lookup in a mapped vDSO image, as a C library performs it.
//!
//! This is a port of musl's `__vdsosym` (`src/internal/vdso.c`), and it is
//! shared verbatim by three callers that would otherwise each have their own
//! near-copy: the build script, which uses it to refuse an image the libc could
//! not use; the integration test, which uses it to find the entry point it then
//! calls for real; and the kernel, for the same reason a build script needs it —
//! to check its own work. Keeping one implementation means "musl can find this
//! symbol" is a single claim verified once, not three approximations that can
//! drift apart.
//!
//! It allocates nothing, indexes nothing without bounds checking, and compiles
//! under `no_std`, because the kernel is one of those three callers. Every
//! malformed input yields `None`.

// Not in the prelude on edition 2018, which this crate shares with the rest of
// the kernel.
use core::convert::{TryFrom, TryInto};

/// Program-header and dynamic-section constants, spelled out because pulling an
/// ELF crate into the kernel for twenty numbers is a bad trade.
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const DT_NULL: u64 = 0;
const DT_HASH: u64 = 4;
const DT_STRTAB: u64 = 5;
const DT_SYMTAB: u64 = 6;
const DT_GNU_HASH: u64 = 0x6fff_fef5;

const SYM_SIZE: usize = 24;

/// `OK_TYPES` in musl: NOTYPE(0), OBJECT(1), FUNC(2), COMMON(5).
const OK_TYPES: u32 = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 5);
/// `OK_BINDS` in musl: GLOBAL(1), WEAK(2), GNU_UNIQUE(10). Bit 0 is STB_LOCAL,
/// which is deliberately excluded.
const OK_BINDS: u32 = (1 << 1) | (1 << 2) | (1 << 10);

fn u16le(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(off..off + 2)?.try_into().ok()?))
}

fn u32le(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

fn u64le(b: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(off..off + 8)?.try_into().ok()?))
}

/// Compares a NUL-terminated string at `off` with `want`, without allocating.
fn name_eq(b: &[u8], off: usize, want: &[u8]) -> bool {
    match b.get(off..off + want.len()) {
        Some(s) if s == want => b.get(off + want.len()) == Some(&0),
        _ => false,
    }
}

/// Resolves `want` to its offset from the start of the image, or `None`.
///
/// The returned value is an offset rather than an address because the image has
/// not been mapped anywhere yet from this function's point of view; add the
/// mapping base to get what musl would return.
///
/// The version argument musl takes ("LINUX_2.6") has no counterpart here: musl
/// skips version checking entirely for images without a `DT_VERDEF`
/// (`if (!verdef) versym = 0;`), and the build script asserts this image has
/// none. Adding one later would silently break every lookup, which is why that
/// assertion exists.
pub fn vdsosym(img: &[u8], want: &[u8]) -> Option<u64> {
    // musl: `base = (size_t)eh + ph->p_offset - ph->p_vaddr` for each PT_LOAD,
    // keeping the last. This image is built with exactly one PT_LOAD at 0/0 so
    // the bias is zero, but compute it the same way rather than assuming — if
    // the invariant ever breaks, this follows the libc into the same answer
    // instead of quietly disagreeing with it.
    if img.get(..4)? != b"\x7fELF" {
        return None;
    }
    let phoff = u64le(img, 32)? as usize;
    let phentsize = u16le(img, 54)? as usize;
    let phnum = u16le(img, 56)? as usize;

    let mut base: i64 = 0;
    let mut dyn_off = None;
    for i in 0..phnum {
        let ph = phoff.checked_add(i.checked_mul(phentsize)?)?;
        match u32le(img, ph)? {
            PT_LOAD => {
                base = u64le(img, ph + 8)? as i64 - u64le(img, ph + 16)? as i64;
            }
            PT_DYNAMIC => dyn_off = Some(u64le(img, ph + 8)? as usize),
            _ => {}
        }
    }

    // Every vaddr below is turned into a file offset with the same bias, which
    // is what makes this work on both an unmapped file image and a mapped one.
    let at = |vaddr: u64| -> Option<usize> { usize::try_from(vaddr as i64 + base).ok() };

    let mut strtab = None;
    let mut symtab = None;
    let mut hash = None;
    let mut gnu_hash = None;
    let mut d = dyn_off?;
    loop {
        let tag = u64le(img, d)?;
        let val = u64le(img, d + 8)?;
        match tag {
            DT_NULL => break,
            DT_STRTAB => strtab = Some(at(val)?),
            DT_SYMTAB => symtab = Some(at(val)?),
            DT_HASH => hash = Some(at(val)?),
            DT_GNU_HASH => gnu_hash = Some(at(val)?),
            _ => {}
        }
        d += 16;
    }
    let (strtab, symtab) = (strtab?, symtab?);

    // musl: `if (hashtab) nsym = hashtab[1]; else if (ghashtab) nsym =
    // count_syms_gnu(ghashtab);` — either table is enough, and this image
    // carries both.
    let nsym = match (hash, gnu_hash) {
        (Some(h), _) => u32le(img, h + 4)? as usize,
        (None, Some(gh)) => count_syms_gnu(img, gh)?,
        (None, None) => return None,
    };

    for i in 0..nsym {
        let sym = symtab.checked_add(i.checked_mul(SYM_SIZE)?)?;
        let info = *img.get(sym + 4)? as u32;
        if (1u32 << (info & 0xf)) & OK_TYPES == 0 {
            continue;
        }
        if (1u32 << (info >> 4)) & OK_BINDS == 0 {
            continue;
        }
        if u16le(img, sym + 6)? == 0 {
            continue; // st_shndx == SHN_UNDEF
        }
        if !name_eq(img, strtab + u32le(img, sym)? as usize, want) {
            continue;
        }
        return u64le(img, sym + 8);
    }
    None
}

/// musl's `count_syms_gnu`: a GNU hash table records no symbol count, so the
/// highest bucket's chain is walked to its terminator to recover one.
fn count_syms_gnu(img: &[u8], gh: usize) -> Option<usize> {
    let nbuckets = u32le(img, gh)? as usize;
    let symoffset = u32le(img, gh + 4)? as usize;
    let bloom_size = u32le(img, gh + 8)? as usize;
    let buckets = gh + 16 + bloom_size.checked_mul(8)?;

    let mut last = 0usize;
    for b in 0..nbuckets {
        last = last.max(u32le(img, buckets + b * 4)? as usize);
    }
    if last < symoffset {
        return Some(symoffset);
    }
    let chains = buckets + nbuckets.checked_mul(4)?;
    let mut i = last;
    while u32le(img, chains + (i - symoffset).checked_mul(4)?)? & 1 == 0 {
        i += 1;
    }
    Some(i + 1)
}
