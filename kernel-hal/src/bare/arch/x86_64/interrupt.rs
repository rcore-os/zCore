use core::ops::Range;

use crate::drivers::all_irq;
use crate::drivers::prelude::{IrqHandler, IrqPolarity, IrqTriggerMode};
use crate::HalResult;

hal_fn_impl! {
    impl mod crate::hal_fn::interrupt {
        fn wait_for_interrupt() {
            use x86_64::instructions::interrupts;
            interrupts::enable_and_hlt();
            interrupts::disable();
        }

        fn is_valid_irq(gsi: usize) -> bool {
            all_irq().first_unwrap().is_valid_irq(gsi)
        }

        fn mask_irq(gsi: usize) -> HalResult {
            Ok(all_irq().first_unwrap().mask(gsi)?)
        }

        fn unmask_irq(gsi: usize) -> HalResult {
            Ok(all_irq().first_unwrap().unmask(gsi)?)
        }

        fn configure_irq(gsi: usize, tm: IrqTriggerMode, pol: IrqPolarity) -> HalResult {
            Ok(all_irq().first_unwrap().configure(gsi, tm, pol)?)
        }

        fn register_irq_handler(gsi: usize, handler: IrqHandler) -> HalResult {
            Ok(all_irq().first_unwrap().register_handler(gsi, handler)?)
        }

        fn unregister_irq_handler(gsi: usize) -> HalResult {
            Ok(all_irq().first_unwrap().unregister(gsi)?)
        }

        fn handle_irq(vector: usize) {
            all_irq().first_unwrap().handle_irq(vector as usize);
        }

        fn msi_alloc_block(requested_irqs: usize) -> HalResult<Range<usize>> {
            Ok(all_irq().first_unwrap().msi_alloc_block(requested_irqs)?)
        }

        fn msi_free_block(block: Range<usize>) -> HalResult {
            Ok(all_irq().first_unwrap().msi_free_block(block)?)
        }

        fn msi_register_handler(
            block: Range<usize>,
            msi_id: usize,
            handler: IrqHandler,
        ) -> HalResult {
            Ok(all_irq().first_unwrap().msi_register_handler(block, msi_id, handler)?)
        }
    }
}
