//! Links `vdso/vdso.c` into the shared object that gets mapped into every Linux
//! process, and — more importantly — proves the result is actually usable
//! before letting the kernel build finish.
//!
//! The verification is the reason this is a build script rather than a
//! checked-in blob. Everything that can go wrong with a vDSO goes wrong
//! silently: a missing `DT_HASH` and musl finds no symbols; a second `PT_LOAD`
//! and it computes the wrong load bias; a stray `R_X86_64_RELATIVE` and the
//! image dereferences an unrelocated GOT slot. None of those fail the link, and
//! all of them are invisible until a guest is running. So after linking we walk
//! the ELF and re-implement musl's `__vdsosym` against it: if
//! `__vdso_clock_gettime` does not resolve here, on the host, in a second, the
//! build stops instead of shipping an image that would quietly do nothing (or
//! quietly fault) forty minutes later inside QEMU.
//!
//! A *missing* compiler is treated differently from a *wrong* image. Without a
//! usable `cc` the crate still builds and simply exposes an empty image, which
//! the kernel reads as "no vDSO" and skips — the guest keeps working, clock
//! reads just keep taking the syscall. A compiler that produces a malformed
//! image, on the other hand, is a bug in this crate and hard-fails.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// The libc's own lookup algorithm, shared with the crate and its tests rather
// than reimplemented here — a second copy could pass while the real one fails.
#[path = "src/elf.rs"]
mod elf;

// ELF constants, spelled out rather than pulled from a crate: this file needs
// perhaps twenty of them and a host dependency in the kernel's build graph is a
// worse trade than a short table.
const ET_DYN: u16 = 3;
const EM_X86_64: u16 = 62;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const SHT_SYMTAB: u32 = 2;
const SHT_RELA: u32 = 4;
const SHT_REL: u32 = 9;
const DT_NULL: u64 = 0;
const DT_HASH: u64 = 4;
const DT_STRTAB: u64 = 5;
const DT_SYMTAB: u64 = 6;
const DT_RELA: u64 = 7;
const DT_RELASZ: u64 = 8;
const DT_REL: u64 = 17;
const DT_RELSZ: u64 = 18;
const DT_GNU_HASH: u64 = 0x6fff_fef5;
const DT_VERDEF: u64 = 0x6fff_fffc;
const DT_VERSYM: u64 = 0x6fff_fff0;

/// The symbols the kernel and the C library both depend on being present.
const REQUIRED_SYMBOLS: &[&str] = &[
    "__vdso_clock_gettime",
    "__vdso_gettimeofday",
    "__vdso_time",
];

fn main() {
    println!("cargo:rerun-if-changed=vdso/vdso.c");
    println!("cargo:rerun-if-changed=vdso/vdso.lds");
    println!("cargo:rerun-if-env-changed=VDSO_CC");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    // `vdso.so` is the link output, kept on disk so it can be inspected with
    // readelf when something looks wrong. `vdso.img` is what the kernel maps.
    let link_path = out_dir.join("vdso.so");
    let image_path = out_dir.join("vdso.img");
    let meta_path = out_dir.join("vdso_meta.rs");

    match build_image(&out_dir, &link_path) {
        Ok(linked) => {
            // Malformed image => hard failure. See the module comment.
            let Verified {
                data_offset,
                load_len,
            } = verify(&linked);
            let image = strip(&linked, load_len);
            fs::write(&image_path, &image).unwrap();
            fs::write(&meta_path, meta_source(true, data_offset, image.len())).unwrap();
        }
        Err(reason) => {
            println!(
                "cargo:warning=linux-vdso: {}. El kernel arrancara sin vDSO y \
                 clock_gettime seguira costando una llamada al sistema.",
                reason
            );
            fs::write(&image_path, b"").unwrap();
            fs::write(&meta_path, meta_source(false, 0, 0)).unwrap();
        }
    }
}

/// Reduces the link output to exactly the bytes that get mapped.
///
/// The link output carries `.symtab`, `.strtab` and the section headers after
/// the loadable region. They are not part of any segment, so mapping them would
/// be meaningless — and actively untidy here, because they would land in the
/// tail of the `_vdso_data` page, the one page this design wants to contain
/// nothing but the clock. `verify` has already read everything the kernel needs
/// out of them, so they can go.
///
/// The section-header fields in the ELF header are zeroed rather than left
/// dangling past the new end of file, so the result stays a well-formed ELF
/// that tools can still read.
fn strip(linked: &[u8], load_len: usize) -> Vec<u8> {
    let mut image = linked[..load_len].to_vec();
    image[40..48].copy_from_slice(&0u64.to_le_bytes()); // e_shoff
    image[58..60].copy_from_slice(&0u16.to_le_bytes()); // e_shentsize
    image[60..62].copy_from_slice(&0u16.to_le_bytes()); // e_shnum
    image[62..64].copy_from_slice(&0u16.to_le_bytes()); // e_shstrndx
    image
}

/// Compiles and links the image, or explains why it could not.
fn build_image(out_dir: &Path, image_path: &Path) -> Result<Vec<u8>, String> {
    // The image is x86_64 machine code with an inline `rdtsc`; there is nothing
    // to build for another architecture, and pretending otherwise would only
    // produce a confusing link error.
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if arch != "x86_64" {
        return Err(format!("arquitectura objetivo {:?} sin vDSO", arch));
    }

    let cc = env::var("VDSO_CC").unwrap_or_else(|_| "cc".to_string());
    let lds = fs::canonicalize("vdso/vdso.lds").map_err(|e| format!("vdso.lds: {}", e))?;
    let src = fs::canonicalize("vdso/vdso.c").map_err(|e| format!("vdso.c: {}", e))?;

    let output = Command::new(&cc)
        .args([
            "-shared",
            "-fPIC",
            // Everything is reached RIP-relative; only the three `__vdso_*`
            // entry points opt back into default visibility. This is what keeps
            // the image free of dynamic relocations.
            "-fvisibility=hidden",
            "-nostdlib",
            "-nostartfiles",
            "-O2",
            "-fno-stack-protector",
            "-fno-asynchronous-unwind-tables",
            "-fno-unwind-tables",
            // No `endbr64`, no `.note.gnu.property`: nothing here is an
            // indirect-branch target and the note would be discarded anyway.
            "-fcf-protection=none",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-std=gnu11",
        ])
        .arg("-Wl,-T")
        .arg(&lds)
        .args([
            // musl accepts either hash table; emitting both also keeps the
            // image usable by a glibc-linked program.
            "-Wl,--hash-style=both",
            "-Wl,-soname=linux-vdso.so.1",
            "-Wl,--build-id=none",
            "-Wl,-z,max-page-size=4096",
            // A dangling reference here would become a relocation nobody
            // processes, so refuse to link one.
            "-Wl,--no-undefined",
        ])
        .arg("-o")
        .arg(image_path)
        .arg(&src)
        .current_dir(out_dir)
        .output()
        .map_err(|e| format!("no se pudo ejecutar {:?}: {}", cc, e))?;

    if !output.status.success() {
        return Err(format!(
            "{:?} fallo ({}):\n{}",
            cc,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let image = fs::read(image_path).map_err(|e| format!("no se pudo leer la imagen: {}", e))?;

    // A cross-hosted `cc` links fine and produces the wrong machine. Catch that
    // here, where it is a "no vDSO" degradation, rather than in `verify`, where
    // it would be a build-stopping error for a perfectly innocent host.
    if image.len() < 20 || u16le(&image, 18) != EM_X86_64 {
        return Err(format!("{:?} no genera codigo x86_64", cc));
    }

    Ok(image)
}

fn meta_source(available: bool, data_offset: usize, len: usize) -> String {
    format!(
        "/// Whether a usable image was linked into this build.\n\
         pub const AVAILABLE: bool = {};\n\
         /// Byte offset of `_vdso_data` from the start of the mapped image.\n\
         pub const DATA_OFFSET: usize = {};\n\
         /// Length of the image in bytes, before rounding up to whole pages.\n\
         pub const IMAGE_LEN: usize = {};\n",
        available, data_offset, len
    )
}

// ---------------------------------------------------------------------------
// ELF verification
// ---------------------------------------------------------------------------

fn u16le(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn u32le(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn u64le(b: &[u8], off: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[off..off + 8]);
    u64::from_le_bytes(v)
}

fn cstr(b: &[u8], off: usize) -> String {
    let end = b[off..].iter().position(|&c| c == 0).unwrap_or(0) + off;
    String::from_utf8_lossy(&b[off..end]).into_owned()
}

/// What `verify` extracts from an image it has accepted.
struct Verified {
    /// Offset of `_vdso_data` from the start of the mapping.
    data_offset: usize,
    /// Bytes of the single `PT_LOAD`, i.e. everything that gets mapped.
    load_len: usize,
}

/// Checks every property the kernel and musl rely on. Panics — with an
/// explanation — on anything that would make the image silently useless.
fn verify(img: &[u8]) -> Verified {
    assert_eq!(&img[..4], b"\x7fELF", "no es un ELF");
    assert_eq!(img[4], 2, "se esperaba ELF64");
    assert_eq!(u16le(img, 16), ET_DYN, "se esperaba ET_DYN");

    let phoff = u64le(img, 32) as usize;
    let phentsize = u16le(img, 54) as usize;
    let phnum = u16le(img, 56) as usize;

    // musl derives the load bias as `p_offset - p_vaddr` of the last PT_LOAD it
    // walks and resolves every symbol against it. With exactly one segment at
    // 0/0 the bias is the mapping base, which is the only arrangement where
    // `AT_SYSINFO_EHDR` pointing at the first page is also correct.
    let mut loads = Vec::new();
    let mut dynamic = None;
    for i in 0..phnum {
        let ph = phoff + i * phentsize;
        match u32le(img, ph) {
            PT_LOAD => loads.push(ph),
            PT_DYNAMIC => dynamic = Some(ph),
            _ => {}
        }
    }
    assert_eq!(
        loads.len(),
        1,
        "la imagen debe tener exactamente un PT_LOAD (tiene {}); \
         con varios, musl calcularia un sesgo de carga distinto de la base del mapeo",
        loads.len()
    );
    let load = loads[0];
    assert_eq!(u64le(img, load + 8), 0, "PT_LOAD.p_offset debe ser 0");
    assert_eq!(u64le(img, load + 16), 0, "PT_LOAD.p_vaddr debe ser 0");
    let filesz = u64le(img, load + 32);
    let memsz = u64le(img, load + 40);
    assert_eq!(
        filesz, memsz,
        "PT_LOAD.p_memsz != p_filesz: la imagen tiene .bss y nadie la pondria a cero"
    );

    let dynamic = dynamic.expect("falta PT_DYNAMIC");
    let dyn_off = u64le(img, dynamic + 8) as usize;
    let dyn_sz = u64le(img, dynamic + 32) as usize;

    let mut tags = std::collections::BTreeMap::new();
    let mut i = 0;
    while i + 16 <= dyn_sz {
        let tag = u64le(img, dyn_off + i);
        let val = u64le(img, dyn_off + i + 8);
        if tag == DT_NULL {
            break;
        }
        tags.insert(tag, val);
        i += 16;
    }

    // Because the single PT_LOAD sits at 0/0, a virtual address is also a file
    // offset, which is what lets the checks below index `img` directly.
    let strtab = *tags.get(&DT_STRTAB).expect("falta DT_STRTAB") as usize;
    let symtab = *tags.get(&DT_SYMTAB).expect("falta DT_SYMTAB") as usize;
    assert!(
        tags.contains_key(&DT_HASH) || tags.contains_key(&DT_GNU_HASH),
        "falta DT_HASH y DT_GNU_HASH: musl no encontraria ningun simbolo"
    );

    // musl skips version checking outright when there is no DT_VERDEF, which is
    // why this image needs no version script despite musl asking for
    // "LINUX_2.6". If a DT_VERDEF ever appears, that shortcut stops applying
    // and every lookup starts failing the version comparison.
    assert!(
        !tags.contains_key(&DT_VERDEF),
        "DT_VERDEF presente: musl dejaria de ignorar las versiones y \
         __vdsosym fallaria salvo que se defina la version LINUX_2.6"
    );
    let _ = DT_VERSYM;

    for (tag, name) in [
        (DT_RELA, "DT_RELA"),
        (DT_RELASZ, "DT_RELASZ"),
        (DT_REL, "DT_REL"),
        (DT_RELSZ, "DT_RELSZ"),
    ] {
        assert!(
            tags.get(&tag).copied().unwrap_or(0) == 0,
            "{} presente: la imagen necesita reubicaciones dinamicas y nadie las aplica",
            name
        );
    }

    // Section headers: the relocation cross-check above only sees what the
    // dynamic table advertises, and `.symtab` is where `_vdso_data` lives.
    let shoff = u64le(img, 40) as usize;
    let shentsize = u16le(img, 58) as usize;
    let shnum = u16le(img, 60) as usize;
    let shstrndx = u16le(img, 62) as usize;
    let shstr = u64le(img, shoff + shstrndx * shentsize + 24) as usize;

    let mut data_offset = None;
    for i in 0..shnum {
        let sh = shoff + i * shentsize;
        let name = cstr(img, shstr + u32le(img, sh) as usize);
        let sh_type = u32le(img, sh + 4);
        let sh_size = u64le(img, sh + 32);

        if (sh_type == SHT_RELA || sh_type == SHT_REL) && sh_size != 0 {
            panic!(
                "seccion de reubicaciones {} con {} bytes: compilar con \
                 -fvisibility=hidden deberia haberlas eliminado",
                name, sh_size
            );
        }

        if sh_type == SHT_SYMTAB {
            let strtab_idx = u32le(img, sh + 40) as usize;
            let symstr = u64le(img, shoff + strtab_idx * shentsize + 24) as usize;
            let off = u64le(img, sh + 24) as usize;
            let entsize = u64le(img, sh + 56) as usize;
            for s in 0..(sh_size as usize / entsize) {
                let sym = off + s * entsize;
                if cstr(img, symstr + u32le(img, sym) as usize) == "_vdso_data" {
                    data_offset = Some(u64le(img, sym + 8) as usize);
                }
            }
        }
    }

    let data_offset = data_offset.expect(
        "no se encuentra el simbolo _vdso_data en .symtab; \
         la imagen no debe pasar por strip",
    );
    assert_eq!(
        data_offset % 4096,
        0,
        "_vdso_data en {:#x} no esta alineado a pagina",
        data_offset
    );
    // The struct itself must be in the file, and nothing may follow it inside
    // its page: the kernel hands that page out as the writable clock mirror, so
    // anything sharing it would be writable too. The tail of the page is
    // implicitly zero and does not appear in the file, hence the upper bound on
    // `filesz` rather than a requirement that a whole page be present.
    assert!(
        data_offset + core::mem::size_of::<u64>() * 3 <= filesz as usize,
        "_vdso_data en {:#x} no cabe en la imagen ({} bytes)",
        data_offset,
        filesz
    );
    assert!(
        (filesz as usize) <= data_offset + 4096,
        "la imagen sigue mas alla de la pagina de _vdso_data ({} bytes tras {:#x}); \
         esa pagina debe contener solo los datos del reloj",
        filesz,
        data_offset
    );

    // The payoff: musl's own algorithm, run here. Everything above narrows down
    // *why* a lookup might fail; this says whether it does.
    let _ = (strtab, symtab);
    for name in REQUIRED_SYMBOLS {
        let addr = elf::vdsosym(img, name.as_bytes()).unwrap_or_else(|| {
            panic!(
                "__vdsosym no resuelve {:?}: la libc no usaria la vDSO",
                name
            )
        });
        assert!(
            addr != 0 && addr < filesz,
            "{:?} resuelve a {:#x}, fuera de la imagen",
            name,
            addr
        );
    }

    println!(
        "cargo:warning=linux-vdso: {} bytes mapeables, _vdso_data en {:#x}, \
         {} simbolos verificados con el algoritmo de musl",
        filesz,
        data_offset,
        REQUIRED_SYMBOLS.len()
    );

    Verified {
        data_offset,
        load_len: filesz as usize,
    }
}
