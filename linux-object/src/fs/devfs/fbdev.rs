//! Implement INode for framebuffer

use alloc::sync::Arc;
use core::{any::Any, convert::From};

use kernel_hal::drivers::prelude::{ColorFormat, DisplayInfo};
use kernel_hal::drivers::scheme::DisplayScheme;
use kernel_hal::vm::{GenericPageTable, PageTable};
use rcore_fs::vfs::*;
use rcore_fs_devfs::DevFS;
use zircon_object::vm::{page_aligned, pages, VmObject};

use crate::error::{LxError, LxResult};

// IOCTLs
const FBIOGET_VSCREENINFO: u32 = 0x4600;
const FBIOGET_FSCREENINFO: u32 = 0x4602;

/// no hardware accelerator
const FB_ACCEL_NONE: u32 = 0;

/// Frambuffer type.
#[repr(u32)]
#[allow(dead_code)]
#[derive(Debug, Copy, Clone)]
pub enum FbType {
    /// Packed Pixels
    PackedPixels = 0,
    /// Non interleaved planes
    Planes = 1,
    /// Interleaved planes
    InterleavedPlanes = 2,
    /// Text/attributes
    Text = 3,
    /// EGA/VGA planes
    VgaPlanes = 4,
    /// Type identified by a V4L2 FOURCC
    FourCC = 5,
}

impl Default for FbType {
    fn default() -> Self {
        Self::PackedPixels
    }
}

/// Framebuffer visual type.
#[repr(u32)]
#[allow(dead_code)]
#[derive(Debug, Copy, Clone)]
pub enum FbVisual {
    /// Monochr. 1=Black 0=White
    Mono01 = 0,
    /// Monochr. 1=White 0=Black
    Mono10 = 1,
    /// True color
    TrueColor = 2,
    /// Pseudo color (like atari)
    PseudoColor = 3,
    /// Direct color
    DirectColor = 4,
    /// Pseudo color readonly
    StaticPseudoColor = 5,
    /// Visual identified by a V4L2 FOURCC
    FourCC = 6,
}

impl Default for FbVisual {
    fn default() -> Self {
        Self::Mono01
    }
}

/// Fixed screen info, defines the properties of a card that are created when a
/// mode is set and can’t be changed otherwise.
#[repr(C)]
#[derive(Debug, Default)]
pub struct FbFixScreeninfo {
    /// identification string eg "TT Builtin"
    id: [u8; 16],
    /// Start of frame buffer mem (physical address)
    smem_start: u64,
    /// Length of frame buffer mem
    smem_len: u32,
    /// see [`FbType`]
    fb_type: FbType,
    /// Interleave for interleaved Planes
    type_aux: u32,
    /// see [`FbVisual`]
    visual: FbVisual,
    /// zero if no hardware panning
    xpanstep: u16,
    /// zero if no hardware panning
    ypanstep: u16,
    /// zero if no hardware ywrap
    ywrapstep: u16,
    /// length of a line in bytes
    line_length: u32,
    /// Start of Memory Mapped I/O (physical address)
    mmio_start: u64,
    /// Length of Memory Mapped I/O
    mmio_len: u32,
    /// Indicate to driver which specific chip/card we have
    accel: u32,
    /// see FB_CAP_*
    capabilities: u16,
    /// Reserved for future compatibility
    _reserved: [u16; 2],
}

impl From<DisplayInfo> for FbFixScreeninfo {
    fn from(info: DisplayInfo) -> Self {
        let smem_start =
            if let Ok((paddr, _, _)) = PageTable::from_current().query(info.fb_base_vaddr) {
                paddr as u64
            } else {
                u64::MAX
            };
        Self {
            smem_start,
            smem_len: info.fb_size as u32,
            fb_type: FbType::PackedPixels,
            visual: FbVisual::TrueColor,
            line_length: info.width * info.format.bytes() as u32,
            mmio_start: 0,
            mmio_len: 0,
            accel: FB_ACCEL_NONE,
            ..Default::default()
        }
    }
}

/// Interpretation of offset for color fields: All offsets are from the right,
/// inside a "pixel" value, which is exactly 'bits_per_pixel' wide (means: you
/// can use the offset as right argument to <<). A pixel afterwards is a bit
/// stream and is written to video memory as that unmodified.
#[repr(C)]
#[derive(Debug, Default)]
pub struct FbBitfield {
    /// beginning of bitfield
    offset: u32,
    /// length of bitfield
    length: u32,
    /// != 0 : Most significant bit is right
    msb_right: u32,
}

/// Variable screen info, describe the features of a video card that are user defined.
#[repr(C)]
#[derive(Debug, Default)]
pub struct FbVarScreeninfo {
    /// visible resolution x
    xres: u32,
    /// visible resolution y
    yres: u32,
    /// virtual resolution x
    xres_virtual: u32,
    /// virtual resolution y
    yres_virtual: u32,
    /// offset from virtual to visible x
    xoffset: u32,
    /// offset from virtual to visible y
    yoffset: u32,

    /// guess what
    bits_per_pixel: u32,
    /// 0 = color, 1 = grayscale, >1 = FOURCC
    grayscale: u32,
    /// red channel. bitfield in fb mem if true color, else only length is significant.
    red: FbBitfield,
    /// green channel
    green: FbBitfield,
    /// blue channel
    blue: FbBitfield,
    /// transparency
    transp: FbBitfield,

    /// != 0 Non standard pixel format
    nonstd: u32,

    /// see FB_ACTIVATE_*
    activate: u32,

    /// height of picture in mm
    height: u32,
    /// width of picture in mm
    width: u32,
    /// (OBSOLETE) see fb_info.flags
    accel_flags: u32,

    /* Timing: All values in pixclocks, except pixclock (of course) */
    /// pixel clock in ps (pico seconds)
    pixclock: u32,
    /// time from sync to picture
    left_margin: u32,
    /// time from picture to sync
    right_margin: u32,
    /// time from sync to picture
    upper_margin: u32,
    lower_margin: u32,
    /// length of horizontal sync
    hsync_len: u32,
    /// length of vertical sync
    vsync_len: u32,
    /// see FB_SYNC_*
    sync: u32,
    /// see FB_VMODE_*
    vmode: u32,
    /// angle we rotate counter clockwise
    rotate: u32,
    /// colorspace for FOURCC-based modes
    colorspace: u32,
    /// Reserved for future compatibility
    _reserved: [u32; 4],
}

impl From<DisplayInfo> for FbVarScreeninfo {
    fn from(info: DisplayInfo) -> Self {
        let (rl, gl, bl, al, ro, go, bo, ao) = match info.format {
            ColorFormat::RGB332 => (3, 3, 2, 0, 5, 3, 0, 0),
            ColorFormat::RGB565 => (5, 6, 5, 0, 11, 5, 0, 0),
            ColorFormat::RGB888 => (8, 8, 8, 0, 16, 8, 0, 0),
            ColorFormat::ARGB8888 => (8, 8, 8, 8, 16, 8, 0, 24),
        };
        Self {
            xres: info.width,
            yres: info.height,
            xres_virtual: info.width,
            yres_virtual: info.height,
            xoffset: 0,
            yoffset: 0,
            bits_per_pixel: info.format.depth() as u32,
            blue: FbBitfield {
                offset: bo,
                length: bl,
                msb_right: 0,
            },
            green: FbBitfield {
                offset: go,
                length: gl,
                msb_right: 0,
            },
            red: FbBitfield {
                offset: ro,
                length: rl,
                msb_right: 0,
            },
            transp: FbBitfield {
                offset: ao,
                length: al,
                msb_right: 0,
            },
            ..Default::default()
        }
    }
}

/// Framebuffer device
pub struct FbDev {
    display: Arc<dyn DisplayScheme>,
    inode_id: usize,
}

impl FbDev {
    pub fn new(display: Arc<dyn DisplayScheme>) -> Self {
        Self {
            display,
            inode_id: DevFS::new_inode_id(),
        }
    }

    pub fn get_vmo(&self, offset: usize, len: usize) -> LxResult<Arc<VmObject>> {
        let info = self.display.info();
        if !page_aligned(offset) || offset >= info.fb_size {
            return Err(LxError::EINVAL);
        }
        let paddr = FbFixScreeninfo::from(info).smem_start;
        if paddr == u64::MAX {
            return Err(LxError::ENOMEM);
        }
        let len = len.min(info.fb_size - offset);
        Ok(VmObject::new_physical(paddr as usize + offset, pages(len)))
    }
}

impl INode for FbDev {
    #[allow(unsafe_code)]
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<usize> {
        info!(
            "fbdev read_at: offset={:#x} buf_len={:#x}",
            offset,
            buf.len()
        );

        let info = self.display.info();
        if offset >= info.fb_size {
            return Ok(0);
        }
        let len = buf.len().min(info.fb_size - offset);
        let fb = self.display.fb();
        buf[..len].copy_from_slice(&fb[offset..offset + len]);
        Ok(len)
    }

    #[allow(unsafe_code)]
    fn write_at(&self, offset: usize, buf: &[u8]) -> Result<usize> {
        info!(
            "fbdev write_at: offset={:#x} buf_len={:#x}",
            offset,
            buf.len()
        );

        let info = self.display.info();
        if offset >= info.fb_size {
            return Ok(0);
        }
        let len = buf.len().min(info.fb_size - offset);
        let mut fb = self.display.fb();
        fb[offset..offset + len].copy_from_slice(&buf[..len]);
        Ok(len)
    }

    fn poll(&self) -> Result<PollStatus> {
        Ok(PollStatus {
            // TOKNOW and TODO
            read: true,
            write: false,
            error: false,
        })
    }

    fn metadata(&self) -> Result<Metadata> {
        Ok(Metadata {
            dev: 1,
            inode: self.inode_id,
            size: 0,
            blk_size: 0,
            blocks: 0,
            atime: Timespec { sec: 0, nsec: 0 },
            mtime: Timespec { sec: 0, nsec: 0 },
            ctime: Timespec { sec: 0, nsec: 0 },
            type_: FileType::CharDevice,
            mode: 0o660,
            nlinks: 1,
            uid: 0,
            gid: 0,
            rdev: make_rdev(0x1d, 0),
        })
    }

    #[allow(unsafe_code)]
    fn io_control(&self, cmd: u32, data: usize) -> Result<usize> {
        match cmd {
            FBIOGET_FSCREENINFO => {
                let dst = unsafe { &mut *(data as *mut FbFixScreeninfo) };
                *dst = self.display.info().into();
                Ok(0)
            }
            FBIOGET_VSCREENINFO => {
                let dst = unsafe { &mut *(data as *mut FbVarScreeninfo) };
                *dst = self.display.info().into();
                Ok(0)
            }
            _ => {
                warn!("use never support ioctl !");
                Err(FsError::NotSupported)
            }
        }
    }

    fn as_any_ref(&self) -> &dyn Any {
        self
    }
}
