use crate::{config::MAX_CORE_NUM, utils::mpsc_queue::MpscQueue};
use alloc::vec::Vec;

const REASON_SIZE: usize = 16;

static mut IPI_REASON0: [IpiEntry; REASON_SIZE] = [0; REASON_SIZE];
static mut IPI_REASON1: [IpiEntry; REASON_SIZE] = [0; REASON_SIZE];
static mut IPI_REASON2: [IpiEntry; REASON_SIZE] = [0; REASON_SIZE];
static mut IPI_REASON3: [IpiEntry; REASON_SIZE] = [0; REASON_SIZE];
static mut IPI_REASON4: [IpiEntry; REASON_SIZE] = [0; REASON_SIZE];
static mut IPI_REASON5: [IpiEntry; REASON_SIZE] = [0; REASON_SIZE];
static mut IPI_REASON6: [IpiEntry; REASON_SIZE] = [0; REASON_SIZE];
static mut IPI_REASON7: [IpiEntry; REASON_SIZE] = [0; REASON_SIZE];

pub type IpiEntry = usize;
type IRQueue = MpscQueue<'static, IpiEntry>;

lazy_static::lazy_static! {
    static ref IPI_QUEUE: [IRQueue; MAX_CORE_NUM] = [
        IRQueue::new(unsafe {&mut IPI_REASON0} ),
        IRQueue::new(unsafe {&mut IPI_REASON1} ),
        IRQueue::new(unsafe {&mut IPI_REASON2} ),
        IRQueue::new(unsafe {&mut IPI_REASON3} ),
        IRQueue::new(unsafe {&mut IPI_REASON4} ),
        IRQueue::new(unsafe {&mut IPI_REASON5} ),
        IRQueue::new(unsafe {&mut IPI_REASON6} ),
        IRQueue::new(unsafe {&mut IPI_REASON7} ),
    ];
}

pub(crate) fn ipi_queue(cpuid: usize) -> &'static IRQueue {
    &IPI_QUEUE[cpuid]
}

pub(crate) fn ipi_reason() -> Vec<usize> {
    let cpu_id = crate::cpu::cpu_id() as usize;
    let queue = ipi_queue(cpu_id);
    queue.consume_entrys().iter().map(|entry| entry.1).collect()
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum IpiReason {
    Invalid,
    MockBlock { block_info: usize },
    TlbShutdown { vpn: usize }, // unused
}

/// usize : 64bit
/// |  type reason : 4bit  |   ipi info : 60bit   |
///
/// MockBlock info : 60bit
/// |  reserved : 60 bit  |
///

const TYPE_SHIFT: usize = 60;
const TYPE_INVALID: usize = 0x0;
const TYPE_MOCK_BLOCK: usize = 0x1;
const TYPE_TLB_SHUTDOWN: usize = 0x2;

impl From<IpiEntry> for IpiReason {
    fn from(r: IpiEntry) -> Self {
        let ipi_type = r >> TYPE_SHIFT;
        let ipi_info = r & 0x000FFFFFFFFFFFFF;
        match ipi_type {
            TYPE_MOCK_BLOCK => Self::MockBlock {
                block_info: ipi_info,
            },
            TYPE_TLB_SHUTDOWN => Self::TlbShutdown { vpn: ipi_info },
            _ => Self::Invalid,
        }
    }
}

impl From<IpiReason> for IpiEntry {
    fn from(reason: IpiReason) -> Self {
        match reason {
            IpiReason::MockBlock { block_info: info } => (TYPE_MOCK_BLOCK << TYPE_SHIFT) | info,
            IpiReason::TlbShutdown { vpn: info } => (TYPE_TLB_SHUTDOWN << TYPE_SHIFT) | info,
            IpiReason::Invalid => 0,
        }
    }
}
