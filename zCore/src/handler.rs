use kernel_hal::{KernelHandler, MMUFlags};
use zircon_object::object::KernelObject;
use zircon_object::task::Thread;

use super::memory;

pub struct ZcoreKernelHandler;

impl KernelHandler for ZcoreKernelHandler {
    fn frame_alloc(&self) -> Option<usize> {
        memory::frame_alloc(1, 0)
    }

    fn frame_alloc_contiguous(&self, frame_count: usize, align_log2: usize) -> Option<usize> {
        memory::frame_alloc(frame_count, align_log2)
    }

    fn frame_dealloc(&self, paddr: usize) {
        memory::frame_dealloc(paddr)
    }

    fn handle_page_fault(&self, fault_vaddr: usize, access_flags: MMUFlags) {
        // Guard: very low addresses (null-pointer dereference with a field offset)
        // are never valid user or kernel mappings — they indicate a use-after-free
        // or corrupted pointer somewhere. Attempting to resolve them through the
        // thread's vmar will itself fault (the vmar/process may be the freed
        // object), causing re-entrant page faults that cascade across all CPUs.
        // Catch them early and report without touching any thread/process state.
        if fault_vaddr < 0x1000 {
            kernel_hal::console::console_write_fmt(format_args!(
                "\n[KERNEL PAGE FAULT] vaddr={:#x} flags={:?} rip={:#x} \
                 (null-range fault -- not retriable, halting)\n",
                fault_vaddr,
                access_flags,
                kernel_hal::kstats::last_fault_rip(),
            ));
            // [diag] Name the interrupted thread/process, if any. Deliberately
            // NOT resolving the fault through its vmar (see the comment above
            // this guard) -- that walks page tables and can itself fault if
            // the vmar/process is the corrupted/freed object. `.name()` only
            // reads a String already owned by a live Arc from the current
            // cpu's own thread-pointer slot (never derived from whatever
            // corrupted the call that led here), so it carries none of that
            // risk. The last several [761] captures all had unreliable
            // raw-stack-scan backtraces (false leads resolving into .rodata
            // tables or into the middle of unrelated functions, not real call
            // sites) -- knowing WHICH process/thread was running narrows the
            // hunt even when the backtrace below doesn't.
            if let Some(thread) = kernel_hal::thread::get_current_thread() {
                if let Ok(thread) = thread.downcast::<Thread>() {
                    kernel_hal::console::console_write_fmt(format_args!(
                        "[diag] running thread: {:?} \"{}\" in process \"{}\"\n",
                        thread.id(),
                        thread.name(),
                        thread.proc().name(),
                    ));
                }
            }
            // [diag] This is the exact signature of the RIP-lands-outside-.text
            // corruption under investigation (issue #761): a corrupted fn-ptr or
            // vtable jumps/calls into the null range. Name the caller before
            // halting -- silently recovering here trades a crash-loop for
            // never seeing this again, which loses the only lead to the root
            // cause. See the shared helper's doc comment.
            print_fault_backtrace();
            // MUST panic, not return: a page fault handler that returns
            // without fixing the mapping just gets IRET'd back to the exact
            // same faulting instruction, which re-faults on the exact same
            // dereference immediately -- an unbounded retry loop, not a
            // recovery. Two reproductions on the reporter's machine hit a
            // Double Fault instead of this branch's own diagnostic output;
            // the most likely explanation is that loop exhausting whatever
            // stack services repeated exception delivery (a return here was
            // never actually safe, independent of anything this session's
            // diagnostic edits touched).
            panic!(
                "null-range kernel page fault: vaddr(0x{:x}) flags({:?})",
                fault_vaddr, access_flags
            );
        }

        if let Some(thread) = kernel_hal::thread::get_current_thread() {
            if let Ok(thread) = thread.downcast::<Thread>() {
                let vmar = thread.proc().vmar();
                if let Err(err) = vmar.handle_page_fault(fault_vaddr, access_flags) {
                    // Loud on the *graphic* console (panic prints to serial only,
                    // invisible on a headless-but-monitor'd bring-up box). A kernel
                    // fault here during a syscall is almost always a driver
                    // dereferencing an address the user vmar can't resolve -- e.g.
                    // the vendored NVIDIA RM touching an unmapped/mismapped MMIO or
                    // heap pointer. The fault address + flags name the culprit
                    // directly instead of leaving a silent frozen `cat`.
                    kernel_hal::console::console_write_fmt(format_args!(
                        "\n[KERNEL PAGE FAULT] vaddr={:#x} flags={:?} err={:?} rip={:#x} \
                         (unresolved against the faulting thread's user vmar -- \
                         likely a kernel-side driver bug, not a userspace fault)\n",
                        fault_vaddr,
                        access_flags,
                        err,
                        kernel_hal::kstats::last_fault_rip(),
                    ));
                    // [diag] Same frame-pointer/raw-stack walk as the kernel-private
                    // branch below -- this fault has a current thread, but the RIP
                    // landing outside .text (e.g. a corrupted fn-ptr/vtable jumping
                    // into .rodata) is exactly the case where naming the *caller*
                    // matters most. See the shared helper's doc comment.
                    print_fault_backtrace();
                    panic!(
                        "handle kernel page fault error: {:?} vaddr(0x{:x}) flags({:?})",
                        err, fault_vaddr, access_flags
                    );
                }
            }
        } else {
            kernel_hal::console::console_write_fmt(format_args!(
                "\n[KERNEL PAGE FAULT] vaddr={:#x} flags={:?} rip={:#x} \
                 (no current thread -- fault in kernel-private context)\n",
                fault_vaddr,
                access_flags,
                kernel_hal::kstats::last_fault_rip(),
            ));
            print_fault_backtrace();
            panic!(
                "page fault from kernel private address 0x{:x}, flags = {:?}, rip = {:#x}",
                fault_vaddr,
                access_flags,
                kernel_hal::kstats::last_fault_rip(),
            );
        }
    }

    fn memory_usage(&self) -> (usize, usize) {
        memory::stats()
    }
}

/// [diag] Walk the frame-pointer chain from the faulting instruction and print
/// the return addresses, then fall back to a raw stack scan for kernel code
/// pointers. The wild write reproduced by `cat /proc/self/exe` faulted inside
/// compiler_builtins `set_bytes` (memset) writing to a corrupted kernel
/// destination; its own frame has no useful name, so the CALLER chain printed
/// here is what names the code that handed memset the bad pointer/length.
/// Same idea applies to an RIP that lands outside `.text` entirely (a
/// corrupted function pointer/vtable): the faulting frame itself is garbage,
/// but its caller's return address on the stack usually still is not. Uses
/// the spin/blocking serial writer so this survives even as the panic that
/// follows re-faults on a corrupted stack. rbp==0 or a scan that leaves the
/// plausible kernel range simply stops the walk.
fn print_fault_backtrace() {
    let rbp0 = kernel_hal::kstats::last_fault_rbp();
    let rsp0 = kernel_hal::kstats::last_fault_rsp();
    kernel_hal::console::serial_write_fmt_spin(format_args!(
        "[kfault-bt] rbp={:#x} rsp={:#x} walking frames:\n",
        rbp0, rsp0,
    ));
    // Two separate bounds, deliberately different widths:
    //  - `plausible` (rbp walk): kept at the original, narrower 256 MiB. This
    //    loop chases whatever value is STORED at each frame -- if that chain
    //    is corrupted (as observed: captures so far show it going nowhere
    //    useful within 1-2 hops), an untrusted `saved_rbp` could point
    //    anywhere; a tighter bound limits how far a bad chain can wander
    //    before this loop's own gate stops it from dereferencing further.
    //  - `plausible_sp` (below): only ever applied to `rsp0` itself and small
    //    fixed offsets from it (the stack scan advances by 8 bytes at a time,
    //    at most 4 KiB total) -- addresses that stay local to a known-live
    //    pointer regardless of how wide this bound is, so widening it here
    //    doesn't carry the rbp-walk's wander risk. See its own comment below
    //    for why it needed widening at all.
    let plausible = |a: u64| a >= 0xffff_ff00_0000_0000 && a < 0xffff_ff00_1000_0000;
    let mut rbp = rbp0;
    let mut i = 0usize;
    while i < 24 && plausible(rbp) && (rbp & 0x7) == 0 {
        // SAFETY: rbp is a validated, 8-aligned kernel-range address; a
        // frame is [saved_rbp, return_addr]. If the chain is corrupt the
        // reads may fault, but we are already panicking.
        let saved_rbp = unsafe { core::ptr::read_volatile(rbp as *const u64) };
        let ret = unsafe { core::ptr::read_volatile((rbp + 8) as *const u64) };
        kernel_hal::console::serial_write_fmt_spin(format_args!(
            "[kfault-bt]   #{:02} ret={:#x} (rbp={:#x})\n",
            i, ret, rbp,
        ));
        if saved_rbp <= rbp {
            break; // frame pointers must strictly increase up the stack
        }
        rbp = saved_rbp;
        i += 1;
    }
    // Also raw-scan the stack for kernel code pointers as a fallback when the
    // frame chain is broken (memset is a leaf that may omit rbp).
    kernel_hal::console::serial_write_fmt_spin(format_args!(
        "[kfault-bt] raw stack scan from rsp:\n",
    ));
    let mut sp = rsp0 & !0x7;
    // Wider bound for everything below: `sp` only ever advances by 8 bytes at
    // a time from `rsp0` (at most 4 KiB total across the whole scan loop), so
    // it stays local to a known-live pointer no matter how wide this bound
    // is -- unlike the rbp walk above, widening it doesn't risk chasing an
    // untrusted value far off into unmapped memory. `plausible(w)` below
    // never dereferences `w`, only reports it, so it's equally safe to use
    // here. See the top-of-function comment for why this needed widening at
    // all: two real captures showed a live rsp being rejected by the
    // original 256 MiB bound.
    let plausible_sp = |a: u64| a >= 0xffff_ff00_0000_0000 && a < 0xffff_ff01_0000_0000;
    // [diag] The single most reliable value here: if the fault was an EXECUTE
    // fault from an indirect `call` through a corrupted fn-ptr/vtable slot,
    // the CPU already pushed the return address (right after that `call`)
    // BEFORE loading the bad target into rip -- so [rsp0] IS that caller's
    // return address, less prone to the false leads a "does this look like a
    // kernel pointer" filter can produce on VALUE reads deeper in the scan.
    // Still gated on `plausible_sp(sp)` (the ADDRESS, not the value) though:
    // sp itself came from the trap frame, but if the underlying bug corrupts
    // more than just one fn-ptr, rsp could be garbage too, and dereferencing
    // an unmapped address here would fault again while already handling a
    // fault -- an #PF-during-#PF is one of the CPU's own double-fault
    // triggers (see the Double Fault this exact bug produced once already).
    if plausible_sp(sp) {
        let top = unsafe { core::ptr::read_volatile(sp as *const u64) };
        kernel_hal::console::serial_write_fmt_spin(format_args!(
            "[kfault-bt]   [rsp0]={:#x} <- likely the bad call's return address\n",
            top,
        ));
    } else {
        kernel_hal::console::serial_write_fmt_spin(format_args!(
            "[kfault-bt]   rsp0={:#x} is itself out of the plausible kernel range -- \
             not dereferencing it (avoids a #PF-during-#PF -> double fault)\n",
            sp,
        ));
    }
    let mut found = 0usize;
    let mut scanned = 0usize;
    while found < 20 && scanned < 512 && plausible_sp(sp) {
        let w = unsafe { core::ptr::read_volatile(sp as *const u64) };
        if plausible_sp(w) {
            kernel_hal::console::serial_write_fmt_spin(format_args!(
                "[kfault-bt]   @{:#x} = {:#x}\n",
                sp, w,
            ));
            found += 1;
        }
        sp += 8;
        scanned += 1;
    }
}
