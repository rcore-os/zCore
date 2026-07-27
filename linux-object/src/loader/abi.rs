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
