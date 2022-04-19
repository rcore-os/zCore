#![allow(dead_code)]
#![allow(clippy::identity_op)]

use trapframe::TrapFrame;
use cortex_a::registers::{ESR_EL1, FAR_EL1};
use tock_registers::interfaces::Readable;

#[no_mangle]
pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
    let info = Info {
        source: Source::from(tf.trap_num & 0xffff),
        kind: Kind::from((tf.trap_num >> 16) & 0xffff)
    };
    debug!("Exception from {:?}", info.source);
    match info.kind {
        Kind::Synchronous => {
            sync_handler(tf);
        },
        Kind::Irq => {
            crate::interrupt::handle_irq(0);
        },
        _ => {
            panic!("Unsupported exception type: {:?}, TrapFrame: {:?}", info.kind, tf);
        }
    }
    debug!("Exception end");
}

fn sync_handler(tf: &mut TrapFrame) {
    let esr = ESR_EL1.extract();
    match esr.read_as_enum(ESR_EL1::EC) {
        Some(ESR_EL1::EC::Value::Unknown) => {
            panic!(
                "Unknown exception @ {:#x}, kernel killed it.",
                tf.elr
            );
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

#[derive(Debug, Eq, PartialEq)]
pub enum IrqHandlerResult {
    Reschedule,
    NoReschedule,
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum Kind {
    Synchronous = 0,
    Irq = 1,
    Fiq = 2,
    SError = 3,
}

impl Kind {
    pub fn from(x: usize) -> Kind {
        match x {
            x if x == Kind::Synchronous as usize => Kind::Synchronous,
            x if x == Kind::Irq as usize => Kind::Irq,
            x if x == Kind::Fiq as usize => Kind::Fiq,
            x if x == Kind::SError as usize => Kind::SError,
            _ => panic!("bad kind"),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum Source {
    CurrentSpEl0 = 0,
    CurrentSpElx = 1,
    LowerAArch64 = 2,
    LowerAArch32 = 3,
}

impl Source {
    pub fn from(x: usize) -> Source {
        match x {
            x if x == Source::CurrentSpEl0 as usize => Source::CurrentSpEl0,
            x if x == Source::CurrentSpElx as usize => Source::CurrentSpElx,
            x if x == Source::LowerAArch64 as usize => Source::LowerAArch64,
            x if x == Source::LowerAArch32 as usize => Source::LowerAArch32,
            _ => panic!("bad kind"),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub struct Info {
    pub source: Source,
    pub kind: Kind,
}
