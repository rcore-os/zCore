use crate::thread::Tcb;

use crate::object::*;
use crate::types::*;
use crate::error::*;
use crate::vm;
use slab::Slab;
use core::cell::UnsafeCell;
use alloc::boxed::Box;
use core::mem::{self, ManuallyDrop};
use crate::sys;
use crate::control;
use crate::futex::FMutex;

const KT_VM_START: usize = 0x7fff00000000usize;

// XXX: Keep this in sync with the layout of `KtVm`.
const KT_VM_ITEM_SIZE: usize = 131072; // 128K

struct KtVmTracker {
    slots: Slab<()>,
}

static KVT: FMutex<KtVmTracker> = FMutex::new(KtVmTracker {
    slots: Slab::new(),
});

#[repr(C, align(4096))]
struct KtVm {
    ipc_buffer: [u8; 4096],
    tls: [u8; 8192],

    /// Stack guard.
    stack_guard: [u8; 4096],

    stack: [u8; 16384],
}

impl KtVm {
    const fn offset_stack_guard() -> usize {
        4096 + 8192
    }

    const fn offset_stack() -> usize {
        4096 + 8192 + 4096
    }

    const fn offset_end() -> usize {
        4096 + 8192 + 4096 + 16384
    }
}

struct KtVmRef {
    backing: *mut UnsafeCell<KtVm>,
}

unsafe impl Send for KtVmRef {}
unsafe impl Sync for KtVmRef {}

impl KtVmRef {
    fn new() -> KernelResult<KtVmRef> {
        let mut kvt = KVT.lock();
        let next_index = kvt.slots.insert(());
        let next_start = KT_VM_START + KT_VM_ITEM_SIZE * next_index;

        // XXX: Keep this in sync with the layout of `KtVm`.
        // The lock to `vm::K` must be acquired & released within the scope of `kvt`. Otherwise there'll be deadlock.
        let mut kvm = vm::K.lock();
        let alloc_result = 
            kvm.allocate_region(next_start..next_start + KtVm::offset_stack_guard()).and_then(|_| {
                kvm.allocate_region(next_start + KtVm::offset_stack()..next_start + KtVm::offset_end())
            });
        drop(kvm);
        match alloc_result {
            Ok(()) =>  {},
            Err(e) => {
                kvt.slots.remove(next_index);
                return Err(e);
            }
        }
        Ok(KtVmRef {
            backing: next_start as _,
        })
    }

    unsafe fn get(&self) -> *mut KtVm {
        (*self.backing).get()
    }
}

impl Drop for KtVmRef {
    fn drop(&mut self) {
        let start = self.backing as usize;
        let index = (start - KT_VM_START) / KT_VM_ITEM_SIZE;

        // XXX: Keep this in sync with the layout of `KtVm`.
        let mut kvm = vm::K.lock();
        kvm.release_region(start);
        kvm.release_region(start + KtVm::offset_stack());
        drop(kvm);

        KVT.lock().slots.remove(index);
    }
}


pub struct KernelThread {
    tcb: ManuallyDrop<Tcb>,
    vm: ManuallyDrop<KtVmRef>,
}

impl Drop for KernelThread {
    fn drop(&mut self) {
        unreachable!("KernelThread::drop");
    }
}

impl KernelThread {
    pub unsafe fn drop_from_control_thread(mut self) {
        // `tcb` must be dropped before `vm`
        ManuallyDrop::drop(&mut self.tcb);
        ManuallyDrop::drop(&mut self.vm);
        mem::forget(self);
    }

    fn start(callback: Box<FnOnce() + Send>) -> KernelResult<()> {
        let tcb = Tcb::new()?;
        let vm = KtVmRef::new()?;

        let ipc_buffer_addr = unsafe {
            (&(*vm.get()).ipc_buffer) as *const _ as usize
        };
        let ipc_page_cap = vm::K.lock().page_at(ipc_buffer_addr).expect("KernelThread::new: failed to find page for ipc buffer").object();

        // Calculate address for init material
        let init_material_place = unsafe {
            let stack = &mut (*vm.get()).stack;
            let stack_len = stack.len();
            mem::transmute::<&mut u8, &mut Option<Box<KtInitMaterial>>>(
                &mut stack[stack_len - mem::size_of::<*mut KtInitMaterial>()]
            )
        };

        // Set TCB properties
        unsafe {
            tcb.prepare_as_kernel_thread(
                _khal_sel4_kt_ll_entry as usize,
                init_material_place as *mut _ as usize,
                ipc_buffer_addr, ipc_page_cap,
            )?;
            tcb.set_priority(sys::L4BRIDGE_MAX_PRIO as u8)?;
        }

        // Now we have the new `KernelThread` ready
        let kt = KernelThread {
            tcb: ManuallyDrop::new(tcb),
            vm: ManuallyDrop::new(vm),
        };

        // Write init material
        *init_material_place = Some(Box::new(KtInitMaterial {
            kt,
            callback,
        }));

        // Resume. We can't fail here.
        if init_material_place.as_mut().unwrap().kt.tcb.resume().is_err() {
            panic!("KernelThread::new: cannot resume new thread");
        }

        // FIXME: Without this line, the new thread runs *very, very* slow. Why?
        crate::thread::yield_now();

        Ok(())
    }
}

pub fn spawn<F: FnOnce() + Send + 'static>(f: F) -> KernelResult<()> {
    KernelThread::start(Box::new(f))
}

#[repr(C)]
struct KtInitMaterial {
    kt: KernelThread,
    callback: Box<FnOnce() + Send>,
}

#[no_mangle]
unsafe extern "C" fn _khal_sel4_kt_hl_entry(material: Box<KtInitMaterial>) -> ! {
    let vm = &*material.kt.vm.get();
    let tls_addr = &vm.tls as *const _ as usize;
    let tls_size = vm.tls.len();
    let ipc_buffer_addr = &vm.ipc_buffer as *const _ as usize;
    sys::l4bridge_setup_tls(tls_addr, tls_size, ipc_buffer_addr);

    // Initialize local context.
    crate::thread::LocalContext::init_current();

    // Ensure that `material` is dropped
    let callback;
    let kt;

    {
        let material = material;
        callback = material.callback;
        kt = material.kt;
    }

    callback();

    crate::thread::LocalContext::drop_current();
    control::exit_thread(kt);
}

extern "C" {
    fn _khal_sel4_kt_ll_entry();
}

global_asm!(r#"
_khal_sel4_kt_ll_entry:
movq 0(%rsp), %rdi
jmp _khal_sel4_kt_hl_entry
"#);
