use crate::thread::Tcb;

use crate::object::*;
use crate::types::*;
use crate::error::*;
use crate::vm;
use crate::sync::YieldMutex;
use slab::Slab;
use core::cell::UnsafeCell;
use alloc::boxed::Box;
use core::mem;
use alloc::sync::Arc;
use crate::sys;

const KT_VM_START: usize = 0x7fff00000000usize;

// XXX: Keep this in sync with the layout of `KtVm`.
const KT_VM_ITEM_SIZE: usize = 131072; // 128K

struct KtVmTracker {
    slots: Slab<()>,
}

static KVT: YieldMutex<KtVmTracker> = YieldMutex::new(KtVmTracker {
    slots: Slab::new(),
});

#[repr(C, align(4096))]
struct KtVm {
    ipc_buffer: [u8; 4096],
    tls: [u8; 16384],

    /// Stack guard.
    unused: [u8; 4096],

    stack: [u8; 65536],
}

impl KtVm {
    const fn offset_unused() -> usize {
        4096 + 16384
    }

    const fn offset_stack() -> usize {
        4096 + 16384 + 4096
    }

    const fn offset_end() -> usize {
        4096 + 16384 + 4096 + 65536
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
        let mut kvm = vm::K.lock();
        let alloc_result = 
            kvm.allocate_region(next_start..next_start + KtVm::offset_unused()).and_then(|_| {
                kvm.allocate_region(next_start + KtVm::offset_stack()..next_start + KtVm::offset_end())
            });
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

        KVT.lock().slots.remove(index);
    }
}


pub struct KernelThread {
    tcb: Tcb,
    vm: KtVmRef,
}

impl KernelThread {
    pub fn new(callback: Box<FnOnce(Arc<KernelThread>)>) -> KernelResult<Arc<KernelThread>> {
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
            mem::transmute::<&mut u8, &mut *mut KtInitMaterial>(
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
            tcb.set_priority(CPtr(sys::L4BRIDGE_STATIC_CAP_TCB), sys::L4BRIDGE_MAX_PRIO as u8)?;
        }

        // Now we have the new `KernelThread` ready
        let kt = Arc::new(KernelThread {
            tcb,
            vm,
        });

        // Write init material
        let init_material = Box::into_raw(Box::new(KtInitMaterial {
            kt: kt.clone(),
            callback,
        }));
        *init_material_place = init_material;

        // Resume. We can't fail here.
        if kt.tcb.resume().is_err() {
            panic!("KernelThread::new: cannot resume new thread");
        }

        Ok(kt)
    }
}

#[repr(C)]
struct KtInitMaterial {
    kt: Arc<KernelThread>,
    callback: Box<FnOnce(Arc<KernelThread>)>,
}

#[no_mangle]
unsafe extern "C" fn _khal_sel4_kt_hl_entry(material: Box<KtInitMaterial>) -> ! {
    let vm = &*material.kt.vm.get();
    let tls_addr = &vm.tls as *const _ as usize;
    let tls_size = vm.tls.len();
    let ipc_buffer_addr = &vm.ipc_buffer as *const _ as usize;
    sys::l4bridge_setup_tls(tls_addr, tls_size, ipc_buffer_addr);
    (material.callback)(material.kt.clone());
    panic!("_khal_sel4_kt_hl_entry: callback returned");
}

extern "C" {
    fn _khal_sel4_kt_ll_entry();
}

global_asm!(r#"
_khal_sel4_kt_ll_entry:
movq 0(%rsp), %rdi
jmp _khal_sel4_kt_hl_entry
"#);
