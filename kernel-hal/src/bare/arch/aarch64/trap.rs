#![allow(dead_code)]
#![allow(clippy::identity_op)]

use crate::imp::config::*;
use crate::{Info, Kind, Source};
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
                phys_to_virt(GICC_BASE),
                phys_to_virt(GICD_BASE),
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
            panic!("Unknown exception @ {:#x}, kernel killed it.", tf.elr);
            // CurrentTask::get().exit(-1);
        }
        Some(ESR_EL1::EC::Value::SVC64) => {
            debug!("syscall...");
            // tf.r[0] = syscall(tf.r[8] as _, [tf.r[0] as _, tf.r[1] as _, tf.r[2] as _], tf) as u64
        }
        Some(ESR_EL1::EC::Value::DataAbortLowerEL)
        | Some(ESR_EL1::EC::Value::DataAbortCurrentEL) => {
            let iss = esr.read(ESR_EL1::ISS);
            panic!(
                "Data Abort @ {:#x}, FAR = {:#x}, ISS = {:#x}, kernel killed it.",
                tf.elr,
                FAR_EL1.get(),
                iss
            );
            // CurrentTask::get().exit(-1);
        }
        Some(ESR_EL1::EC::Value::InstrAbortLowerEL)
        | Some(ESR_EL1::EC::Value::InstrAbortCurrentEL) => {
            let iss = esr.read(ESR_EL1::ISS);
            panic!(
                "Instruction Abort @ {:#x}, FAR = {:#x}, ISS = {:#x}, kernel killed it.",
                tf.elr,
                FAR_EL1.get(),
                iss
            );
            // CurrentTask::get().exit(-1);
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
