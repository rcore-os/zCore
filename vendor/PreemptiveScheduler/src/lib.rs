#![no_std]
#![feature(allocator_api)]
#![feature(get_mut_unchecked)]
// `generators`/`generator_trait` fue renombrado y luego retirado; en nightlies nuevas se usa coroutines.
#![feature(coroutines, coroutine_trait, yield_expr)]
// some interfaces is still under developing
#![allow(dead_code)]

cfg_if::cfg_if! {
  if #[cfg(target_arch = "x86_64")] {
      #[path = "arch/x86_64/mod.rs"]
      #[macro_use]
      mod arch;
  } else if #[cfg(target_arch = "riscv64")] {
      #[path = "arch/riscv64/mod.rs"]
      #[macro_use]
      mod arch;
  } else if #[cfg(target_arch = "aarch64")] {
      #[path = "arch/aarch64/mod.rs"]
      #[macro_use]
      mod arch;
  }
}

extern crate alloc;
#[macro_use]
extern crate log;

mod context;
mod diag;
mod executor;
mod runtime;
mod task_collection;
mod waker_page;

pub use executor::sched_stats;
pub use executor::{
    alloc_overlaps_live_stack, hard_guard_executor_counts, overlapping_live_stack,
    set_stack_guard_hooks, set_stack_quarantine_enabled, set_stack_quarantine_hooks,
    stack_guard_hooks_registered, GUARD_SIZE, STACK_SIZE, TOP_GUARD_SIZE,
};
pub use runtime::{
    abandon_current_executor, abandon_current_task, abandon_executor_for_sp, attribute_fault_stack_ptrs, check_current_executor_canary,
    check_current_executor_stack_proximity, current_stack_top_looks_null,
    current_executor_abandonable, current_task_abandonable, fault_sp_abandonable, handle_timeout,
    heap_smash_suspected, irq_on_idle_executor, irq_should_skip_dyn_dispatch,
    irq_should_skip_heavy_work, note_heap_smash_suspected, run_until_idle, sched_yield,
    set_idle_callback, set_resched_ipi_sender, set_wakeup_preempt, spawn, spawn_with_affinity,
    take_need_resched, wakeup_preempt_enabled, wakeup_preempt_stats, warm_runtimes,
    FaultStackAttr, StackAttrHit, StackPtrRegion,
};

#[macro_export]
macro_rules! run_with_intr_saved_on {
    ($($statements:stmt)*) => {
        let enable = crate::arch::intr_get();
        if !enable {
          crate::arch::intr_on();
        }
        $($statements)*
        if !enable {
          crate::arch::intr_off();
        }
    };
}

#[macro_export]
macro_rules! run_with_intr_saved_off {
    ($($statements:stmt)*) => {
        let enable = crate::arch::intr_get();
        if enable {
            crate::arch::intr_off();
        }
        $($statements)*
        if enable {
            crate::arch::intr_on();
        }
    };
}
