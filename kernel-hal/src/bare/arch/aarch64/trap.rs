#![allow(dead_code)]
#![allow(clippy::identity_op)]

use crate::{Info, KCONFIG, Kind, Source};
use cortex_a::registers::{ESR_EL1, FAR_EL1};
use tock_registers::interfaces::Readable;
use trapframe::TrapFrame;
use zcore_drivers::irq::gic_400::get_irq_num;

#[no_mangle]
pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
    let info = Info {
        source: Source::from(tf.trap_num & 0xffff),
        kind: Kind::from((tf.trap_num >> 16) & 0xffff),
    };
    trace!("Exception from {:?}", info.source);
    match info.kind {
        Kind::Synchronous => {
            sync_handler(tf);
        }
        Kind::Irq => {
            use crate::hal_fn::mem::phys_to_virt;
            crate::interrupt::handle_irq(get_irq_num(
                phys_to_virt(KCONFIG.gic_base + 0x1_0000),
                phys_to_virt(KCONFIG.gic_base),
            ));
        }
        _ => {
            panic!(
                "Unsupported exception type: {:?}, TrapFrame: {:?}",
                info.kind, tf
            );
        }
    }
    trace!("Exception end");
}

fn sync_handler(tf: &mut TrapFrame) {
    let esr = ESR_EL1.extract();
    match esr.read_as_enum(ESR_EL1::EC) {
        Some(ESR_EL1::EC::Value::Unknown) => {
            panic!("Unknown exception @ {:#x}", tf.elr);
        }
        Some(ESR_EL1::EC::Value::SVC64) => {
            debug!("syscall...");
        }
        Some(ESR_EL1::EC::Value::DataAbortLowerEL)
        | Some(ESR_EL1::EC::Value::DataAbortCurrentEL) => {
            let iss = esr.read(ESR_EL1::ISS);
            panic!(
                "Data Abort @ {:#x}, FAR = {:#x}, ISS = {:#x}",
                tf.elr,
                FAR_EL1.get(),
                iss
            );
        }
        Some(ESR_EL1::EC::Value::InstrAbortLowerEL)
        | Some(ESR_EL1::EC::Value::InstrAbortCurrentEL) => {
            let iss = esr.read(ESR_EL1::ISS);
            panic!(
                "Instruction Abort @ {:#x}, FAR = {:#x}, ISS = {:#x}",
                tf.elr,
                FAR_EL1.get(),
                iss
            );
        }
        _ => {
            panic!(
                "Unsupported synchronous exception @ {:#x}: ESR = {:#x} (EC {:#08b}, ISS {:#x})",
                tf.elr,
                esr.get(),
                esr.read(ESR_EL1::EC),
                esr.read(ESR_EL1::ISS),
            );
        }
    }
}
