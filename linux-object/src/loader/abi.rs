//! Process init info

use alloc::collections::btree_map::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::mem::align_of;
use core::ops::Deref;

/// process init information
pub struct ProcInitInfo {
    /// args strings
    pub args: Vec<String>,
    /// environment strings
    pub envs: Vec<String>,
    /// auxiliary
    pub auxv: BTreeMap<u8, usize>,
}

impl ProcInitInfo {
    /// push process init information into stack
    pub fn push_at(&self, stack_top: usize) -> Stack {
        // We will build the stack from top to bottom.
        // The strings and random bytes go first (highest addresses).
        let mut writer = Stack::new(stack_top);

        // 1. Random bytes for AT_RANDOM (16 bytes)
        let random_bytes = [0u8; 16]; // TODO: use real random
        writer.push_slice(&random_bytes);
        let random_ptr = writer.sp;

        // 2. Program name for AT_EXECFN
        writer.push_str(&self.args[0]);
        let execfn_ptr = writer.sp;

        // 3. Environment strings
        let env_ptrs: Vec<_> = self
            .envs
            .iter()
            .map(|arg| {
                writer.push_str(arg.as_str());
                writer.sp
            })
            .collect();

        // 4. Argv strings
        let arg_ptrs: Vec<_> = self
            .args
            .iter()
            .map(|arg| {
                writer.push_str(arg.as_str());
                writer.sp
            })
            .collect();

        // Now we prepare the pointer arrays (auxv, envp, argv, argc).
        // These must be contiguous and the final sp (at argc) must be 16-byte aligned.
        let mut table = Vec::new();

        // Argc
        table.push(self.args.len());
        // Argv
        for ptr in arg_ptrs {
            table.push(ptr);
        }
        table.push(0); // NULL
                       // Envp
        for ptr in env_ptrs {
            table.push(ptr);
        }
        table.push(0); // NULL

        // Auxv
        for (&type_, &value) in self.auxv.iter() {
            table.push(type_ as usize);
            table.push(value);
        }
        table.push(AT_RANDOM as usize);
        table.push(random_ptr);
        table.push(AT_EXECFN as usize);
        table.push(execfn_ptr);
        table.push(0); // AT_NULL type
        table.push(0); // AT_NULL value

        // To ensure the final sp is 16-byte aligned:
        // Current sp is where strings ended.
        // We will push `table.len()` usize elements.
        // final_sp = sp - table.len() * 8.
        // We want final_sp % 16 == 0.
        let mut sp = writer.sp;
        sp &= !0x7; // Ensure 8-byte alignment first
        let table_size = table.len() * 8;
        if !(sp - table_size).is_multiple_of(16) {
            sp -= 8; // Add 8 bytes of padding
        }
        writer.sp = sp;

        // Push the table. Since push_usize_slice decrements sp first and then
        // copies the slice, table[0] (argc) will end up at the lowest address (the new sp).
        writer.push_usize_slice(&table);

        writer
    }
}

/// program stack
pub struct Stack {
    /// stack pointer
    sp: usize,
    /// stack top
    stack_top: usize,
    /// stack data buffer
    data: Vec<u8>,
}

impl Stack {
    /// create a stack
    #[allow(clippy::uninit_vec, unsafe_code)] // FIXME: 这是什么东西？！为什么要这么做？！实在难以理解！！
    fn new(sp: usize) -> Self {
        let mut data = Vec::with_capacity(0x4000);
        unsafe { data.set_len(0x4000) };
        Stack {
            sp,
            stack_top: sp,
            data,
        }
    }
    /// push slice into stack
    #[allow(unsafe_code)]
    fn push_slice<T: Copy>(&mut self, vs: &[T]) {
        self.push_slice_aligned(vs, align_of::<T>());
    }

    #[allow(unsafe_code)]
    fn push_slice_aligned<T: Copy>(&mut self, vs: &[T], align: usize) {
        self.sp -= core::mem::size_of_val(vs);
        self.sp -= self.sp % align;
        assert!(self.stack_top - self.sp <= self.data.len());
        let offset = self.data.len() - (self.stack_top - self.sp);
        unsafe {
            core::slice::from_raw_parts_mut(self.data.as_mut_ptr().add(offset) as *mut T, vs.len())
        }
        .copy_from_slice(vs);
    }

    fn push_usize_slice(&mut self, vs: &[usize]) {
        self.push_slice_aligned(vs, align_of::<usize>());
    }

    /// push str into stack
    fn push_str(&mut self, s: &str) {
        self.push_slice(b"\0");
        self.push_slice(s.as_bytes());
    }

    /// Overwrite already-pushed bytes at absolute user address `addr`.
    ///
    /// Used by the FreeBSD stack builder to back-patch the `ps_strings` block
    /// (pushed high, before the argv/envp arrays exist) once the addresses of
    /// those arrays are known. `addr` must lie within the region already
    /// written (`[sp, stack_top)`).
    #[allow(dead_code)]
    fn write_at(&mut self, addr: usize, bytes: &[u8]) {
        debug_assert!(addr >= self.sp && addr + bytes.len() <= self.stack_top);
        let off = self.data.len() - (self.stack_top - addr);
        self.data[off..off + bytes.len()].copy_from_slice(bytes);
    }
}

/// FreeBSD auxiliary-vector types (`sys/sys/elf_common.h`). Only the entries a
/// FreeBSD `crt1`/libc reads to reach `main` are defined; the low ones
/// (`AT_PHDR`..`AT_ENTRY`) share their numbers with Linux, the rest are
/// FreeBSD-specific.
#[cfg(target_arch = "x86_64")]
mod fbsd_at {
    pub const AT_PHDR: usize = 3;
    pub const AT_PHENT: usize = 4;
    pub const AT_PHNUM: usize = 5;
    pub const AT_PAGESZ: usize = 6;
    pub const AT_BASE: usize = 7;
    pub const AT_ENTRY: usize = 9;
    pub const AT_EXECPATH: usize = 15;
    pub const AT_CANARY: usize = 16;
    pub const AT_CANARYLEN: usize = 17;
    pub const AT_OSRELDATE: usize = 18;
    pub const AT_NCPUS: usize = 19;
    pub const AT_PAGESIZES: usize = 20;
    pub const AT_PAGESIZESLEN: usize = 21;
    pub const AT_STACKPROT: usize = 23;
    pub const AT_EHDRFLAGS: usize = 24;
    pub const AT_HWCAP: usize = 25;
    pub const AT_PS_STRINGS: usize = 32;
    pub const AT_USRSTACKBASE: usize = 35;
    pub const AT_USRSTACKLIM: usize = 36;
    pub const AT_NULL: usize = 0;
}

/// The machine/process values the FreeBSD auxiliary vector carries that the ELF
/// loader computes per image (program headers, entry, interpreter base) plus
/// the few static ones (`__FreeBSD_version`, CPU count).
#[cfg(target_arch = "x86_64")]
pub struct FreebsdAuxv {
    /// `AT_PHDR`: user address of the program-header table.
    pub phdr: usize,
    /// `AT_PHENT`: size of one program-header entry.
    pub phent: usize,
    /// `AT_PHNUM`: number of program-header entries.
    pub phnum: usize,
    /// `AT_BASE`: interpreter load base (0 for a static binary).
    pub base: usize,
    /// `AT_ENTRY`: the program's own entry point.
    pub entry: usize,
    /// `AT_PAGESZ`: page size in bytes.
    pub pagesz: usize,
    /// `AT_EHDRFLAGS`: `e_flags` from the ELF header.
    pub ehdrflags: usize,
    /// `AT_OSRELDATE`: `__FreeBSD_version` we advertise.
    pub osreldate: usize,
    /// `AT_NCPUS`: number of CPUs.
    pub ncpus: usize,
    /// `AT_EXECPATH`: path the program was executed as.
    pub execpath: String,
}

#[cfg(target_arch = "x86_64")]
impl ProcInitInfo {
    /// Build a FreeBSD/amd64 initial stack and return the [`Stack`] image.
    ///
    /// The layout mirrors what FreeBSD's `exec_copyout_strings` produces
    /// (`sys/kern/kern_exec.c`): from the top down — the `ps_strings` block, the
    /// SSP canary, the `execpath` string, the page-size array, the environment
    /// and argument strings, then the `argc / argv[] / NULL / envp[] / NULL /
    /// auxv[]` table. `argc` ends up at the lowest written address, which is the
    /// value the caller loads into both `%rdi` and (aligned) `%rsp`.
    ///
    /// `ps_strings` is pushed first (highest address) but its `argv`/`envp`
    /// pointers are only known after the table is laid out, so it is back-patched
    /// via [`Stack::write_at`].
    pub fn push_at_freebsd(&self, stack_top: usize, aux: &FreebsdAuxv) -> Stack {
        use fbsd_at::*;
        let mut w = Stack::new(stack_top);

        // 1. ps_strings placeholder (32 bytes: argvstr, nargv, envstr, nenv).
        w.push_slice(&[0u8; 32]);
        let ps_strings_ptr = w.sp;

        // 2. SSP canary (16 random bytes).
        let mut canary = [0u8; 16];
        kernel_hal::rand::fill_random(&mut canary);
        w.push_slice(&canary);
        let canary_ptr = w.sp;

        // 3. execpath string.
        w.push_str(&aux.execpath);
        let execpath_ptr = w.sp;

        // 4. page-size array (a single entry: the base page size).
        w.push_slice(&[aux.pagesz as u64]);
        let pagesizes_ptr = w.sp;

        // 5. environment strings, then argument strings.
        let env_ptrs: Vec<usize> = self
            .envs
            .iter()
            .map(|e| {
                w.push_str(e);
                w.sp
            })
            .collect();
        let arg_ptrs: Vec<usize> = self
            .args
            .iter()
            .map(|a| {
                w.push_str(a);
                w.sp
            })
            .collect();

        // 6. The pointer/aux table. AT_PS_STRINGS is already known; AT_ARGV /
        //    AT_ENVV are deliberately omitted (FreeBSD csu reads argc/argv/envp
        //    straight off the stack), which avoids needing post-table addresses
        //    here.
        let mut table: Vec<usize> = Vec::new();
        table.push(self.args.len()); // argc
        table.extend_from_slice(&arg_ptrs);
        table.push(0); // argv NULL
        table.extend_from_slice(&env_ptrs);
        table.push(0); // envp NULL

        let auxv: [(usize, usize); 19] = [
            (AT_EXECPATH, execpath_ptr),
            (AT_PHDR, aux.phdr),
            (AT_PHENT, aux.phent),
            (AT_PHNUM, aux.phnum),
            (AT_BASE, aux.base),
            (AT_ENTRY, aux.entry),
            (AT_PAGESZ, aux.pagesz),
            (AT_OSRELDATE, aux.osreldate),
            (AT_NCPUS, aux.ncpus),
            (AT_CANARY, canary_ptr),
            (AT_CANARYLEN, canary.len()),
            (AT_PAGESIZES, pagesizes_ptr),
            (AT_PAGESIZESLEN, core::mem::size_of::<u64>()),
            (AT_STACKPROT, 0b11), // PROT_READ | PROT_WRITE
            (AT_EHDRFLAGS, aux.ehdrflags),
            (AT_HWCAP, 0),
            (AT_PS_STRINGS, ps_strings_ptr),
            (AT_USRSTACKBASE, stack_top),
            (AT_USRSTACKLIM, 8 * 1024 * 1024),
        ];
        for (ty, val) in auxv {
            table.push(ty);
            table.push(val);
        }
        table.push(AT_NULL);
        table.push(0);

        // 16-byte-align the final argc address, matching push_at().
        let mut sp = w.sp & !0x7;
        let table_size = table.len() * 8;
        if !(sp - table_size).is_multiple_of(16) {
            sp -= 8;
        }
        w.sp = sp;
        w.push_usize_slice(&table);
        let argc_addr = w.sp;

        // 7. Back-patch ps_strings now that the argv/envp arrays have addresses.
        let argv_addr = (argc_addr + 8) as u64;
        // argc(1) + argv[argc] + argv NULL(1), each 8 bytes, lands at envp[0].
        let envp_addr = (argc_addr + 8 * (1 + self.args.len() + 1)) as u64;
        let mut ps = [0u8; 32];
        ps[0..8].copy_from_slice(&argv_addr.to_le_bytes());
        ps[8..12].copy_from_slice(&(self.args.len() as u32).to_le_bytes());
        ps[16..24].copy_from_slice(&envp_addr.to_le_bytes());
        ps[24..28].copy_from_slice(&(self.envs.len() as u32).to_le_bytes());
        w.write_at(ps_strings_ptr, &ps);

        w
    }
}

impl Deref for Stack {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        let offset = self.data.len() - (self.stack_top - self.sp);
        &self.data[offset..]
    }
}

pub const AT_PHDR: u8 = 3;
pub const AT_PHENT: u8 = 4;
pub const AT_PHNUM: u8 = 5;
pub const AT_PAGESZ: u8 = 6;
pub const AT_BASE: u8 = 7;
pub const AT_ENTRY: u8 = 9;
pub const AT_UID: u8 = 11;
pub const AT_EUID: u8 = 12;
pub const AT_GID: u8 = 13;
pub const AT_EGID: u8 = 14;
pub const AT_SECURE: u8 = 23;
pub const AT_RANDOM: u8 = 25;
pub const AT_EXECFN: u8 = 31;
