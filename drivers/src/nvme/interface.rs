use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
// use core::mem::size_of;
use alloc::sync::Arc;
use core::ptr::{read_volatile, write_volatile};

use crate::scheme::{BlockScheme, Scheme};
use crate::DeviceResult;

use lock::Mutex;

use super::nvme_queue::*;

pub struct NvmeInterface {
    name: String,

    admin_queue: Arc<Mutex<NvmeQueue<ProviderImpl>>>,

    io_queues: Vec<Arc<Mutex<NvmeQueue<ProviderImpl>>>>,

    bar: usize,

    irq: usize,
}

impl NvmeInterface {
    pub fn new(bar: usize, irq: usize) -> DeviceResult<NvmeInterface> {
        let admin_queue = Arc::new(Mutex::new(NvmeQueue::new(0, 0)));

        let io_queues = vec![Arc::new(Mutex::new(NvmeQueue::<ProviderImpl>::new(1, 0x8)))];

        let mut interface = NvmeInterface {
            name: String::from("nvme"),
            admin_queue,
            io_queues,
            bar,
            irq,
        };

        interface.init();

        Ok(interface)
    }

    // config admin queue ,io queue
    pub fn init(&mut self) {
        self.nvme_configure_admin_queue();

        self.nvme_alloc_io_queue();
    }

    pub fn get_name_irq(&self) -> (String, usize) {
        (self.name.clone(), self.irq)
    }
}

impl NvmeInterface {
    pub fn nvme_configure_admin_queue(&mut self) {
        let mut admin_queue = self.admin_queue.lock();

        let bar = self.bar;
        let dbs = bar + NVME_REG_DBS;

        let sq_dma_pa = admin_queue.sq_pa as u32;
        let cq_dma_pa = admin_queue.cq_pa as u32;
        let data_dma_pa = admin_queue.data_pa as u64;

        let aqa_low_16 = 31_u16;
        let aqa_high_16 = 31_u16;
        let aqa = (aqa_high_16 as u32) << 16 | aqa_low_16 as u32;
        let aqa_address = bar + NVME_REG_AQA;

        // 将admin queue配置信息写入nvme设备寄存器AQA (admin_queue_attributes)
        unsafe {
            write_volatile(aqa_address as *mut u32, aqa);
        }

        // 将admin queue的sq dma物理地址写入nvme设备上的寄存器ASQ
        let asq_address = bar + NVME_REG_ASQ;
        unsafe {
            write_volatile(asq_address as *mut u32, sq_dma_pa);
        }

        // 将admin queue的cq dma物理地址写入nvme设备上的寄存器ACQ
        let acq_address = bar + NVME_REG_ACQ;
        unsafe {
            write_volatile(acq_address as *mut u32, cq_dma_pa);
        }

        // enable ctrl
        let mut ctrl_config = NVME_CC_ENABLE | NVME_CC_CSS_NVM;
        ctrl_config |= 0 << NVME_CC_MPS_SHIFT;
        ctrl_config |= NVME_CC_ARB_RR | NVME_CC_SHN_NONE;
        ctrl_config |= NVME_CC_IOSQES | NVME_CC_IOCQES;

        unsafe { write_volatile((bar + NVME_REG_CC) as *mut u32, ctrl_config) }

        let _dev_status = unsafe { read_volatile((bar + NVME_REG_CSTS) as *mut u32) };

        // warn!("nvme status {}", _dev_status);

        // config identify
        let mut cmd = NvmeIdentify::new();
        cmd.prp1 = data_dma_pa;
        cmd.command_id = 0x1018; //random number
        cmd.nsid = 1;
        let common_cmd = unsafe { core::mem::transmute(cmd) };

        admin_queue.sq[0].write(common_cmd);
        admin_queue.sq_tail += 1;

        let admin_q_db = dbs + admin_queue.db_offset;
        unsafe { write_volatile(admin_q_db as *mut u32, 1) }

        loop {
            let status = admin_queue.cq[0].read();
            if status.status != 0 {
                // warn!("nvme cq :{:#x?}", status);
                unsafe { write_volatile((admin_q_db + 0x4) as *mut u32, 1) }
                break;
            }
        }
    }

    pub fn nvme_alloc_io_queue(&mut self) {
        let mut admin_queue = self.admin_queue.lock();
        // let io_queue = self.io_queues[0].lock();

        let bar = self.bar;
        let dev_dbs = bar + NVME_REG_DBS;

        let admin_q_db = dev_dbs;

        // nvme_set_queue_count
        let mut cmd = NvmeCommonCommand::new();
        cmd.opcode = 0x09;
        cmd.command_id = 0x2;
        cmd.nsid = 1;
        cmd.cdw10 = 0x7;

        admin_queue.sq[1].write(cmd);
        admin_queue.sq_tail += 1;

        unsafe { write_volatile(admin_q_db as *mut u32, 2) }

        loop {
            let status = admin_queue.cq[1].read();
            if status.status != 0 {
                // warn!("nvme cq :{:#x?}", status);
                unsafe { write_volatile((admin_q_db + 0x4) as *mut u32, 2) }
                break;
            }
        }

        //nvme create cq
        let mut cmd = NvmeCreateCq::new();
        cmd.opcode = 0x05;
        cmd.command_id = 0x3;
        cmd.nsid = 1;
        cmd.prp1 = admin_queue.cq_pa as u64;
        cmd.cqid = 1;
        cmd.qsize = 1023;
        cmd.cq_flags = NVME_QUEUE_PHYS_CONTIG | NVME_CQ_IRQ_ENABLED;

        // let mut cmd = NvmeCommonCommand::new();
        // cmd.opcode = 0x05;
        // cmd.command_id = 0x3;
        // cmd.nsid = 1;
        // cmd.prp1 = admin_queue.cq_pa as u64;
        // cmd.cdw10 = 0x3ff0001;
        // cmd.cdw11 = 0x3;

        let common_cmd = unsafe { core::mem::transmute(cmd) };

        admin_queue.sq[2].write(common_cmd);
        admin_queue.sq_tail += 1;
        unsafe { write_volatile(admin_q_db as *mut u32, 3) }
        loop {
            let status = admin_queue.cq[2].read();
            if status.status != 0 {
                // warn!("nvme cq :{:#x?}", status);
                unsafe { write_volatile((admin_q_db + 0x4) as *mut u32, 3) }
                break;
            }
        }

        // nvme create sq
        let mut cmd = NvmeCreateSq::new();
        cmd.opcode = 0x01;
        cmd.command_id = 0x4;
        cmd.nsid = 1;
        cmd.prp1 = admin_queue.sq_pa as u64;
        cmd.sqid = 1;
        cmd.qsize = 1023;
        cmd.sq_flags = 0x1;
        cmd.cqid = 0x1;

        // let mut cmd = NvmeCommonCommand::new();
        // cmd.opcode = 0x01;
        // cmd.command_id = 0x2018;
        // cmd.nsid = 1;
        // cmd.prp1 = admin_queue.sq_pa as u64;
        // cmd.cdw10 = 0x3ff0001;
        // cmd.cdw11 = 0x10001;

        let common_cmd = unsafe { core::mem::transmute(cmd) };

        // write command to sq
        admin_queue.sq[3].write(common_cmd);
        admin_queue.sq_tail += 1;

        // write doorbell register
        unsafe { write_volatile(admin_q_db as *mut u32, 4) }

        // wait for command complete
        loop {
            let status = admin_queue.cq[3].read();
            if status.status != 0 {
                // warn!("nvme cq :{:#x?}", status);

                // write doorbell register
                unsafe { write_volatile((admin_q_db + 0x4) as *mut u32, 4) }
                break;
            }
        }
    }
}

impl BlockScheme for NvmeInterface {
    // 每个NVMe命令中有两个域：PRP1和PRP2，Host就是通过这两个域告诉SSD数据在内存中的位置或者数据需要写入的地址
    // 首先对prp1进行读写，如果数据还没完，就看数据量是不是在一个page内，在的话，只需要读写prp2内存地址就可以了，数据量大于1个page，就需要读出prp list

    // 由于只读一块, 小于一页, 所以只需要prp1
    // prp1 = dma_addr
    // prp2 = 0

    // prp设置
    // uboot中对应实现 nvme_setup_prps
    // linux中对应实现 nvme_pci_setup_prps

    // SLBA = start logical block address
    // length = 1 = 512B
    // 1 SLBA = 512B
    fn read_block(&self, block_id: usize, read_buf: &mut [u8]) -> DeviceResult {
        let io_queue = self.io_queues[0].lock();
        let db_offset = io_queue.db_offset;
        let mut admin_queue = self.admin_queue.lock();

        let bar = self.bar;

        let dbs = bar + NVME_REG_DBS;
        // let db_offset = io_queue.db_offset;

        // 这里dma addr 就是buffer的地址
        let ptr = read_buf.as_mut_ptr();
        let addr = virt_to_phys(ptr as usize);

        // build nvme read command
        let mut cmd = NvmeRWCommand::new_read_command();
        cmd.nsid = 1;
        cmd.prp1 = addr as u64;
        cmd.command_id = 101;
        cmd.length = 1;
        cmd.slba = block_id as u64;

        //transfer to common command
        let common_cmd = unsafe { core::mem::transmute(cmd) };

        let tail = admin_queue.sq_tail;

        // write command to sq
        admin_queue.sq[tail].write(common_cmd);
        admin_queue.sq_tail += 1;

        // write doorbell register
        unsafe { write_volatile((dbs + db_offset) as *mut u32, (tail + 1) as u32) }

        // wait for command complete
        loop {
            let status = admin_queue.cq[tail].read();
            if status.status != 0 {
                // warn!("nvme cq :{:#x?}", status);

                // write doorbell
                unsafe { write_volatile((dbs + db_offset + 0x4) as *mut u32, (tail + 1) as u32) }
                break;
            }
        }

        // admin_queue.cq_head = admin_queue.sq_tail;

        Ok(())
    }

    // prp1 = write_buf physical address
    // prp2 = 0
    // SLBA = start logical block address
    // length = 1 = 512B
    fn write_block(&self, block_id: usize, write_buf: &[u8]) -> DeviceResult {
        // warn!("write block");
        let io_queue = self.io_queues[0].lock();
        let db_offset = io_queue.db_offset;
        let mut admin_queue = self.admin_queue.lock();
        let bar = self.bar;
        let dbs = bar + NVME_REG_DBS;

        let ptr = write_buf.as_ptr();

        let addr = virt_to_phys(ptr as usize);

        // build nvme write command
        let mut cmd = NvmeRWCommand::new_write_command();
        cmd.nsid = 1;
        cmd.prp1 = addr as u64;
        cmd.length = 1;
        cmd.command_id = 100;
        cmd.slba = block_id as u64;

        // transmute to common command
        let common_cmd = unsafe { core::mem::transmute(cmd) };

        let mut tail = admin_queue.sq_tail;
        if tail > 1023 {
            tail = 0;
        }

        // push command to sq
        admin_queue.sq[tail].write(common_cmd);
        admin_queue.sq_tail += 1;

        // write doorbell register
        unsafe { write_volatile((dbs + db_offset) as *mut u32, (tail + 1) as u32) }

        // wait for command complete
        loop {
            let status = admin_queue.cq[tail].read();
            if status.status != 0 {
                // warn!("nvme cq :{:#x?}", status);

                // write doorbell
                unsafe { write_volatile((dbs + db_offset + 0x4) as *mut u32, (tail + 1) as u32) }
                break;
            }
        }
        Ok(())
    }

    fn flush(&self) -> DeviceResult {
        Ok(())
    }
}

impl Scheme for NvmeInterface {
    fn name(&self) -> &str {
        "nvme"
    }

    fn handle_irq(&self, irq: usize) {
        warn!("nvme device irq {}", irq);
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
//64B
pub struct NvmeCommonCommand {
    opcode: u8,
    flags: u8,
    command_id: u16,
    nsid: u32,
    cdw2: [u32; 2],
    metadata: u64,
    prp1: u64,
    prp2: u64,
    cdw10: u32,
    cdw11: u32,
    cdw12: u32,
    cdw13: u32,
    cdw14: u32,
    cdw15: u32,
}

impl NvmeCommonCommand {
    pub fn new() -> Self {
        Self {
            opcode: 0,
            flags: 0,
            command_id: 0,
            nsid: 0,
            cdw2: [0; 2],
            metadata: 0,
            prp1: 0,
            prp2: 0,
            cdw10: 0,
            cdw11: 0,
            cdw12: 0,
            cdw13: 0,
            cdw14: 0,
            cdw15: 0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NvmeIdentify {
    opcode: u8,
    flags: u8,
    command_id: u16,
    nsid: u32,
    rsvd2: [u64; 2],
    prp1: u64,
    prp2: u64,
    cns: u8,
    rsvd3: u8,
    ctrlid: u16,
    rsvd11: [u8; 3],
    csi: u8,
    rsvd12: [u32; 4],
}

impl NvmeIdentify {
    pub fn new() -> Self {
        Self {
            opcode: 0x06,
            flags: 0,
            command_id: 0x1,
            nsid: 1,
            rsvd2: [0; 2],
            prp1: 0,
            prp2: 0,
            cns: 1,
            rsvd3: 0,
            ctrlid: 0,
            rsvd11: [0; 3],
            csi: 0,
            rsvd12: [0; 4],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NvmeCreateCq {
    pub opcode: u8,
    pub flags: u8,
    pub command_id: u16,
    pub nsid: u32,
    pub rsvd1: [u32; 4],
    pub prp1: u64,
    pub rsvd8: u64,
    pub cqid: u16,
    pub qsize: u16,
    pub cq_flags: u16,
    pub irq_vector: u16,
    pub rsvd12: [u32; 4],
}

impl NvmeCreateCq {
    fn new() -> Self {
        Self {
            opcode: 0x05,
            flags: 0,
            command_id: 0,
            nsid: 0,
            rsvd1: [0; 4],
            prp1: 0,
            rsvd8: 0,
            cqid: 0,
            qsize: 0,
            cq_flags: 0,
            irq_vector: 0,
            rsvd12: [0; 4],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NvmeCreateSq {
    pub opcode: u8,
    pub flags: u8,
    pub command_id: u16,
    pub nsid: u32,
    pub rsvd1: [u32; 4],
    pub prp1: u64,
    pub rsvd8: u64,
    pub sqid: u16,
    pub qsize: u16,
    pub sq_flags: u16,
    pub cqid: u16,
    pub rsvd12: [u32; 4],
}

impl NvmeCreateSq {
    fn new() -> Self {
        Self {
            opcode: 0x01,
            flags: 0,
            command_id: 0,
            nsid: 0,
            rsvd1: [0; 4],
            prp1: 0,
            rsvd8: 0,
            sqid: 0,
            qsize: 0,
            sq_flags: 0,
            cqid: 0,
            rsvd12: [0; 4],
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct NvmeRWCommand {
    pub opcode: u8,
    pub flags: u8,
    pub command_id: u16,
    pub nsid: u32,
    pub rsvd2: u64,
    pub metadata: u64,
    pub prp1: u64,
    pub prp2: u64,
    pub slba: u64,
    pub length: u16,
    pub control: u16,
    pub dsmgmt: u32,
    pub reftag: u32,
    pub apptag: u16,
    pub appmask: u16,
}

impl NvmeRWCommand {
    pub fn new_write_command() -> Self {
        Self {
            opcode: 0x01,
            flags: 0,
            command_id: 0,
            nsid: 0,
            rsvd2: 0,
            metadata: 0,
            prp1: 0,
            prp2: 0,
            slba: 0,
            length: 0,
            control: 0,
            dsmgmt: 0,
            reftag: 0,
            apptag: 0,
            appmask: 0,
        }
    }
    pub fn new_read_command() -> Self {
        Self {
            opcode: 0x02,
            flags: 0,
            command_id: 0,
            nsid: 0,
            rsvd2: 0,
            metadata: 0,
            prp1: 0,
            prp2: 0,
            slba: 0,
            length: 0,
            control: 0,
            dsmgmt: 0,
            reftag: 0,
            apptag: 0,
            appmask: 0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
pub struct NvmeCompletion {
    pub result: u64,
    // pub rsvd: u32,
    pub sq_head: u16,
    pub sq_id: u16,
    pub command_id: u16,
    pub status: u16,
}

// NvmeRegister
pub const NVME_REG_CAP: usize = 0x0000; /* Controller Capabilities */
pub const NVME_REG_VS: usize = 0x0008; /* Version */
pub const NVME_REG_INTMS: usize = 0x000c; /* Interrupt Mask Set */
pub const NVME_REG_INTMC: usize = 0x0010; /* Interrupt Mask Clear */
pub const NVME_REG_CC: usize = 0x0014; /* Controller Configuration */
pub const NVME_REG_CSTS: usize = 0x001c; /* Controller Status */
pub const NVME_REG_NSSR: usize = 0x0020; /* NVM Subsystem Reset */
pub const NVME_REG_AQA: usize = 0x0024; /* Admin Queue Attributes */
pub const NVME_REG_ASQ: usize = 0x0028; /* Admin SQ Base Address */
pub const NVME_REG_ACQ: usize = 0x0030; /* Admin CQ Base Address */
pub const NVME_REG_CMBLOC: usize = 0x0038; /* Controller Memory Buffer Location */
pub const NVME_REG_CMBSZ: usize = 0x003c; /* Controller Memory Buffer Size */
pub const NVME_REG_BPINFO: usize = 0x0040; /* Boot Partition Information */
pub const NVME_REG_BPRSEL: usize = 0x0044; /* Boot Partition Read Select */
pub const NVME_REG_BPMBL: usize = 0x0048; /* Boot Partition Memory Buffer
                                           * Location
                                           */
pub const NVME_REG_CMBMSC: usize = 0x0050; /* Controller Memory Buffer Memory
                                            * Space Control
                                            */
pub const NVME_REG_CRTO: usize = 0x0068; /* Controller Ready Timeouts */
pub const NVME_REG_PMRCAP: usize = 0x0e00; /* Persistent Memory Capabilities */
pub const NVME_REG_PMRCTL: usize = 0x0e04; /* Persistent Memory Region Control */
pub const NVME_REG_PMRSTS: usize = 0x0e08; /* Persistent Memory Region Status */
pub const NVME_REG_PMREBS: usize = 0x0e0c; /* Persistent Memory Region Elasticity
                                            * Buffer Size
                                            */
pub const NVME_REG_PMRSWTP: usize = 0x0e10; /* Persistent Memory Region Sustained
                                             * Write Throughput
                                             */
pub const NVME_REG_DBS: usize = 0x1000; /* SQ 0 Tail Doorbell */

// NVME CONST
pub const NVME_CC_ENABLE: u32 = 1 << 0;
pub const NVME_CC_CSS_NVM: u32 = 0 << 4;
pub const NVME_CC_MPS_SHIFT: u32 = 7;
pub const NVME_CC_ARB_RR: u32 = 0 << 11;
pub const NVME_CC_ARB_WRRU: u32 = 1 << 11;
pub const NVME_CC_ARB_VS: u32 = 7 << 11;
pub const NVME_CC_SHN_NONE: u32 = 0 << 14;
pub const NVME_CC_SHN_NORMAL: u32 = 1 << 14;
pub const NVME_CC_SHN_ABRUPT: u32 = 2 << 14;
pub const NVME_CC_IOSQES: u32 = 6 << 16;
pub const NVME_CC_IOCQES: u32 = 4 << 20;
pub const NVME_CSTS_RDY: u32 = 1 << 0;
pub const NVME_CSTS_CFS: u32 = 1 << 1;
pub const NVME_CSTS_SHST_NORMAL: u32 = 0 << 2;
pub const NVME_CSTS_SHST_OCCUR: u32 = 1 << 2;
pub const NVME_CSTS_SHST_CMPLT: u32 = 2 << 2;

pub const NVME_QUEUE_PHYS_CONTIG: u16 = 1 << 0;
pub const NVME_CQ_IRQ_ENABLED: u16 = 1 << 1;
pub const NVME_SQ_PRIO_URGENT: u16 = 0 << 1;
pub const NVME_SQ_PRIO_HIGH: u16 = 1 << 1;
pub const NVME_SQ_PRIO_MEDIUM: u16 = 2 << 1;
pub const NVME_SQ_PRIO_LOW: u16 = 3 << 1;

pub const NVME_FEAT_ARBITRATION: u32 = 0x01;
pub const NVME_FEAT_POWER_MGMT: u32 = 0x02;
pub const NVME_FEAT_LBA_RANGE: u32 = 0x03;
pub const NVME_FEAT_TEMP_THRESH: u32 = 0x04;
pub const NVME_FEAT_ERR_RECOVERY: u32 = 0x05;
pub const NVME_FEAT_VOLATILE_WC: u32 = 0x06;
pub const NVME_FEAT_NUM_QUEUES: u32 = 0x07;
pub const NVME_FEAT_IRQ_COALESCE: u32 = 0x08;
pub const NVME_FEAT_IRQ_CONFIG: u32 = 0x09;
pub const NVME_FEAT_WRITE_ATOMIC: u32 = 0x0a;
pub const NVME_FEAT_ASYNC_EVENT: u32 = 0x0b;
pub const NVME_FEAT_SW_PROGRESS: u32 = 0x0c;
