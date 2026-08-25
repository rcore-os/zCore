//! File operations
//!
//! - read, pread, readv
//! - write, pwrite, writev
//! - lseek
//! - truncate, ftruncate
//! - sendfile, copy_file_range
//! - sync, fsync, fdatasync
//! - ioctl, fcntl
//! - access, faccessat

use super::*;
use linux_object::{process::FsInfo, time::TimeSpec};

impl Syscall<'_> {
    /// Reads from a specified file using a file descriptor. Before using this call,
    /// you must first obtain a file descriptor using the opensyscall. Returns bytes read successfully.
    /// - fd – file descriptor
    /// - base – pointer to the buffer to fill with read contents
    /// - len – number of bytes to read
    pub async fn sys_read(&self, fd: FileDesc, base: UserOutPtr<u8>, len: usize) -> SysResult {
        info!("read: fd={:?}, base={:?}, len={:#x}", fd, base, len);
        let proc = self.linux_process();
        let file_like = proc.get_file_like(fd)?;

        let chunk_size = len.min(super::SYSCALL_IO_MAX);
        // `is_seekable` only changes the outcome for a multi-chunk request: it
        // decides whether to keep refilling the buffer past one `chunk_size`
        // slice. For a single-chunk read (`len <= chunk_size`, the overwhelming
        // majority) the loop terminates the same way either way, so skip the
        // `metadata()` probe — it would otherwise re-read the inode on every
        // read syscall.
        let is_seekable = if len > chunk_size {
            if let Ok(file) = file_like.clone().downcast_arc::<linux_object::fs::File>() {
                file.metadata()
                    .map(|meta| {
                        meta.type_ == linux_object::fs::vfs::FileType::File
                            || meta.type_ == linux_object::fs::vfs::FileType::BlockDevice
                    })
                    .unwrap_or(false)
            } else {
                false
            }
        } else {
            false
        };
        // Hybrid stack/heap buffer: line-oriented apps (busybox shell, getline,
        // fgetc) drive a stream of small reads — keep those alloc-free. The
        // ~64 KiB ceiling case still goes via the buddy allocator.
        const STACK_BUF: usize = 512;
        let mut stack_buf = [0u8; STACK_BUF];
        let mut heap_buf: alloc::vec::Vec<u8> = if chunk_size > STACK_BUF {
            vec![0u8; chunk_size]
        } else {
            alloc::vec::Vec::new()
        };
        let buf: &mut [u8] = if chunk_size > STACK_BUF {
            &mut heap_buf[..]
        } else {
            &mut stack_buf[..chunk_size]
        };
        let mut read_len = 0;

        while read_len < len {
            let current_len = (len - read_len).min(chunk_size);
            // [diag dd-efault] name the failing layer: is the EFAULT for a
            // >=4 KiB read produced by the file's read (inode/driver) or by
            // the copy-out below?
            let n = file_like.read(&mut buf[..current_len]).await.map_err(|e| {
                kernel_hal::klog_info!(
                    "[read-efault] fd={:?} len={:#x} current={:#x}: file_like.read -> {:?}",
                    fd,
                    len,
                    current_len,
                    e
                );
                e
            })?;
            if n == 0 {
                break;
            }
            // NOTE: an ETX (0x03) -> SIGINT conversion used to live HERE, keyed
            // only on `fd == 0`. That misfired catastrophically for any program
            // whose stdin is NOT a terminal: piping a binary through a pipeline
            // (`cat /bin/busybox | wc -c`, tar/gzip pipelines, X startup
            // scripts) killed the reader with a spurious SIGINT whenever a read
            // chunk happened to start with byte 0x03. Terminal interrupt
            // handling belongs to the line discipline, and both the VT `Stdin`
            // (termios ISIG, stdio.rs) and the PTY slaves (pty.rs, devfs/pty.rs)
            // already convert VINTR into SIGINT for the foreground pgrp
            // themselves — so the syscall-level check was pure downside and was
            // removed.
            base.add(read_len).write_array(&buf[..n]).map_err(|e| {
                kernel_hal::klog_info!(
                    "[read-efault] fd={:?} base={:#x} n={:#x}: write_array -> {:?}",
                    fd,
                    base.as_addr() + read_len,
                    n,
                    e
                );
                e
            })?;
            read_len += n;
            if n < current_len || !is_seekable {
                break;
            }
        }
        Ok(read_len)
    }

    /// Writes to a specified file using a file descriptor. Before using this call,
    /// you must first obtain a file descriptor using the open syscall. Returns bytes written successfully.
    /// - fd – file descriptor
    /// - base – pointer to the buffer write
    /// - len – number of bytes to write
    pub fn sys_write(&self, fd: FileDesc, base: UserInPtr<u8>, len: usize) -> SysResult {
        info!("write: fd={:?}, base={:?}, len={:#x}", fd, base, len);
        // Diagnostic: surface X-server log/error lines into the dmesg ring so the
        // reason a graphics server aborts is visible even without its logfile.
        // Only stdout/stderr (where Xorg and the dynamic linker print — services
        // dup2 them onto their log files, keeping fd 1/2): the 12-needle
        // substring scan used to run on EVERY write of every fd, taxing the
        // hottest syscall in the system for pipes, sockets and data files that
        // can never carry these markers.
        if <FileDesc as Into<i32>>::into(fd) <= 2 {
            if let Ok(peek) = base.as_slice(len.min(512)) {
                tee_x_diag(peek);
            }
        }
        let proc = self.linux_process();
        // [ebadf-write] dbus-daemon dies with "Writing to pipe: Bad file
        // descriptor" on the fd dbus-launch hands it via --print-address, and
        // the execve CLOEXEC sweep is NOT what closes it. write(2) can answer
        // EBADF for two distinct reasons here, so name which one: the fd is
        // absent from the table, or it is present but its access mode is not
        // writable (File::write_at). Error path only — the fast path is
        // untouched.
        let file_like = proc.get_file_like(fd).inspect_err(|_| {
            kernel_hal::klog_info!(
                "[ebadf-write] fd={:?} ABSENT from the fd table (proc={:?}, live fds={:?})",
                fd,
                proc.execute_path(),
                proc.get_files().map(|f| {
                    let mut v: alloc::vec::Vec<i32> = f.keys().map(|k| (*k).into()).collect();
                    v.sort_unstable();
                    v
                })
            );
        })?;
        let chunk_size = len.min(super::SYSCALL_IO_MAX);
        let mut written = 0usize;
        while written < len {
            let n = (len - written).min(chunk_size);
            let res = file_like
                .write(base.add(written).as_slice(n)?)
                .inspect_err(|&e| {
                    if e == LxError::EBADF {
                        let path = file_like
                            .downcast_ref::<linux_object::fs::File>()
                            .map(|f| f.path().clone())
                            .unwrap_or_default();
                        kernel_hal::klog_info!(
                            "[ebadf-write] fd={:?} PRESENT but write refused: path={:?} \
                             flags={:?} (proc={:?})",
                            fd,
                            path,
                            file_like.flags(),
                            proc.execute_path()
                        );
                    }
                });
            let w = match res {
                Ok(w) => w,
                // A later chunk failing (e.g. EAGAIN once the pipe/socket
                // buffer filled) must report the bytes already consumed, not
                // the error — POSIX partial write. Erroring would make the
                // caller resend data the file already took.
                Err(_) if written > 0 => break,
                Err(e) => return Err(e),
            };
            // A write of 0 would otherwise spin forever; stop and report the
            // bytes written so far (short write).
            if w == 0 {
                break;
            }
            written += w;
        }
        Ok(written)
    }

    /// read from or write to a file descriptor at a given offset
    /// reads up to count bytes from file descriptor fd at offset offset
    /// (from the start of the file) into the buffer starting at buf. The file offset is not changed.
    pub async fn sys_pread(
        &self,
        fd: FileDesc,
        base: UserOutPtr<u8>,
        len: usize,
        offset: u64,
    ) -> SysResult {
        info!(
            "pread: fd={:?}, base={:?}, len={}, offset={}",
            fd, base, len, offset
        );
        let proc = self.linux_process();
        let file_like = proc.get_file_like(fd)?;

        let chunk_size = len.min(super::SYSCALL_IO_MAX);
        // Same hybrid buffer as sys_read — short positional reads (e.g. ELF
        // header probes during dlopen, libc pread of small struct slots) hit
        // the stack path.
        const STACK_BUF: usize = 512;
        let mut stack_buf = [0u8; STACK_BUF];
        let mut heap_buf: alloc::vec::Vec<u8> = if chunk_size > STACK_BUF {
            vec![0u8; chunk_size]
        } else {
            alloc::vec::Vec::new()
        };
        let buf: &mut [u8] = if chunk_size > STACK_BUF {
            &mut heap_buf[..]
        } else {
            &mut stack_buf[..chunk_size]
        };
        let mut read_len = 0;

        while read_len < len {
            let current_len = (len - read_len).min(chunk_size);
            let n = file_like
                .read_at(offset + read_len as u64, &mut buf[..current_len])
                .await?;
            if n == 0 {
                break;
            }
            base.add(read_len).write_array(&buf[..n])?;
            read_len += n;
            if n < current_len {
                break;
            }
        }
        Ok(read_len)
    }

    /// writes up to count bytes from the buffer
    /// starting at buf to the file descriptor fd at offset offset. The file offset is not changed.
    pub fn sys_pwrite(
        &self,
        fd: FileDesc,
        base: UserInPtr<u8>,
        len: usize,
        offset: u64,
    ) -> SysResult {
        info!(
            "pwrite: fd={:?}, base={:?}, len={}, offset={}",
            fd, base, len, offset
        );
        self.linux_process()
            .get_file_like(fd)?
            .write_at(offset, base.as_slice(len)?)
    }

    /// works just like read except that multiple buffers are filled.
    /// reads iov_count buffers from the file
    /// associated with the file descriptor fd into the buffers described by iov ("scatter input")
    pub async fn sys_readv(
        &self,
        fd: FileDesc,
        iov_ptr: UserInPtr<IoVecOut>,
        iov_count: usize,
    ) -> SysResult {
        info!("readv: fd={:?}, iov={:?}, count={}", fd, iov_ptr, iov_count);
        let mut iovs = iov_ptr.read_iovecs(iov_count)?;
        let proc = self.linux_process();
        let file_like = proc.get_file_like(fd)?;
        let total_len = iovs.total_len().min(super::SYSCALL_IO_MAX);
        // Mirror the sys_read hybrid buffer: many readv callers (e.g. socket
        // headers + payload split into two small iovecs) request totals well
        // under 512 B, so keep those alloc-free.
        const STACK_BUF: usize = 512;
        let mut stack_buf = [0u8; STACK_BUF];
        let mut heap_buf: alloc::vec::Vec<u8> = if total_len > STACK_BUF {
            vec![0u8; total_len]
        } else {
            alloc::vec::Vec::new()
        };
        let buf: &mut [u8] = if total_len > STACK_BUF {
            &mut heap_buf[..]
        } else {
            &mut stack_buf[..total_len]
        };
        let len = file_like.read(buf).await?;
        iovs.write_from_buf(&buf[..len])?;
        Ok(len)
    }

    /// works just like write except that multiple buffers are written out.
    /// writes iov_count buffers of data described
    /// by iov to the file associated with the file descriptor fd ("gather output").
    ///
    /// There is deliberately NO total-length limit here. Linux caps a writev
    /// only at `MAX_RW_COUNT` (~2 GiB); rejecting large totals with `EINVAL`
    /// instead is what killed every GLX client against the now-working
    /// Xwayland: xcb flushes big requests as `writev(fd, [queue, header,
    /// payload], 3)`, and an uncompressed `PutImage` of a 300×300 window is
    /// ~352 KiB — the old `> SYSCALL_IO_MAX -> EINVAL` answer made xcb mark
    /// the connection dead and the client exit with "XIO: fatal IO error 22
    /// (Invalid argument)" right after its window appeared. Instead, gather
    /// through a bounded kernel buffer in `SYSCALL_IO_MAX` chunks — the same
    /// bounded-copy iteration Linux does — and report a partial count when a
    /// later chunk cannot proceed (POSIX short write; xcb and stdio both
    /// resume from it).
    pub fn sys_writev(
        &self,
        fd: FileDesc,
        iov_ptr: UserInPtr<IoVecIn>,
        iov_count: usize,
    ) -> SysResult {
        info!(
            "writev: fd={:?}, iov={:?}, count={}",
            fd, iov_ptr, iov_count
        );
        let iovs = iov_ptr.read_iovecs(iov_count)?;
        let total = iovs.total_len();
        let proc = self.linux_process();
        let file_like = proc.get_file_like(fd)?;
        let mut buf = vec![0u8; total.min(super::SYSCALL_IO_MAX)];
        let mut written = 0usize;
        while written < total {
            let n = iovs.read_bytes_at(written, &mut buf)?;
            if n == 0 {
                break;
            }
            // stdout/stderr only — see sys_write.
            if <FileDesc as Into<i32>>::into(fd) <= 2 {
                tee_x_diag(&buf[..n]);
            }
            match file_like.write(&buf[..n]) {
                Ok(w) => {
                    written += w;
                    if w < n {
                        break;
                    }
                }
                // Progress already made: report the partial count; the error
                // will surface on the caller's next write. Erroring here
                // instead would make the caller believe NOTHING was written
                // and resend bytes the file already consumed.
                Err(_) if written > 0 => break,
                Err(e) => return Err(e),
            }
        }
        Ok(written)
    }

    /// read multiple buffers from a file descriptor at a given offset
    /// (see [linux man preadv(2)](https://www.man7.org/linux/man-pages/man2/preadv.2.html)).
    ///
    /// Combines `readv` scatter semantics with `pread`'s stateless offset: the
    /// file position is left untouched, which is what makes it safe under
    /// concurrent readers (glibc's stdio unlocked paths and various databases
    /// pick it over `lseek`+`readv` precisely for that atomicity).
    pub async fn sys_preadv(
        &self,
        fd: FileDesc,
        iov_ptr: UserInPtr<IoVecOut>,
        iov_count: usize,
        offset: u64,
    ) -> SysResult {
        info!(
            "preadv: fd={:?}, iov={:?}, count={}, offset={}",
            fd, iov_ptr, iov_count, offset
        );
        let mut iovs = iov_ptr.read_iovecs(iov_count)?;
        let proc = self.linux_process();
        let file_like = proc.get_file_like(fd)?;
        let total_len = iovs.total_len().min(super::SYSCALL_IO_MAX);
        let mut buf = vec![0u8; total_len];
        let len = file_like.read_at(offset, &mut buf).await?;
        iovs.write_from_buf(&buf[..len])?;
        Ok(len)
    }

    /// write multiple buffers to a file descriptor at a given offset
    /// (see [linux man pwritev(2)](https://www.man7.org/linux/man-pages/man2/pwritev.2.html)).
    ///
    /// Gather-output twin of [`sys_preadv`](Self::sys_preadv); the file position
    /// is not changed.
    pub fn sys_pwritev(
        &self,
        fd: FileDesc,
        iov_ptr: UserInPtr<IoVecIn>,
        iov_count: usize,
        offset: u64,
    ) -> SysResult {
        info!(
            "pwritev: fd={:?}, iov={:?}, count={}, offset={}",
            fd, iov_ptr, iov_count, offset
        );
        let iovs = iov_ptr.read_iovecs(iov_count)?;
        // Same chunked gather as sys_writev — no artificial total cap, short
        // write on a mid-stream failure — with the position carried in the
        // explicit offset instead of the file cursor.
        let total = iovs.total_len();
        let proc = self.linux_process();
        let file_like = proc.get_file_like(fd)?;
        let mut buf = vec![0u8; total.min(super::SYSCALL_IO_MAX)];
        let mut written = 0usize;
        while written < total {
            let n = iovs.read_bytes_at(written, &mut buf)?;
            if n == 0 {
                break;
            }
            match file_like.write_at(offset + written as u64, &buf[..n]) {
                Ok(w) => {
                    written += w;
                    if w < n {
                        break;
                    }
                }
                Err(_) if written > 0 => break,
                Err(e) => return Err(e),
            }
        }
        Ok(written)
    }

    /// `preadv2`: [`sys_preadv`](Self::sys_preadv) plus per-call flags
    /// (see [linux man preadv2(2)](https://www.man7.org/linux/man-pages/man2/readv.2.html)).
    ///
    /// An offset of -1 means "use and update the current file position", i.e.
    /// plain `readv`. The RWF_* flags request behaviours this kernel does not
    /// implement (NOWAIT, HIPRI, DSYNC, ...), so any non-zero flag answers
    /// `EOPNOTSUPP` — the documented reply for unsupported flags, which callers
    /// treat as "fall back to preadv". Silently accepting RWF_NOWAIT and then
    /// blocking would be worse than refusing it.
    pub async fn sys_preadv2(
        &self,
        fd: FileDesc,
        iov_ptr: UserInPtr<IoVecOut>,
        iov_count: usize,
        offset: i64,
        flags: usize,
    ) -> SysResult {
        if flags != 0 {
            return Err(LxError::EOPNOTSUPP);
        }
        if offset == -1 {
            return self.sys_readv(fd, iov_ptr, iov_count).await;
        }
        self.sys_preadv(fd, iov_ptr, iov_count, offset as u64).await
    }

    /// `pwritev2`: [`sys_pwritev`](Self::sys_pwritev) plus per-call flags
    /// (see [linux man pwritev2(2)](https://www.man7.org/linux/man-pages/man2/readv.2.html)).
    ///
    /// Offset -1 falls back to `writev` semantics; non-zero flags answer
    /// `EOPNOTSUPP` for the same reason as [`sys_preadv2`](Self::sys_preadv2).
    pub fn sys_pwritev2(
        &self,
        fd: FileDesc,
        iov_ptr: UserInPtr<IoVecIn>,
        iov_count: usize,
        offset: i64,
        flags: usize,
    ) -> SysResult {
        if flags != 0 {
            return Err(LxError::EOPNOTSUPP);
        }
        if offset == -1 {
            return self.sys_writev(fd, iov_ptr, iov_count);
        }
        self.sys_pwritev(fd, iov_ptr, iov_count, offset as u64)
    }

    /// repositions the offset of the open file associated with the file descriptor fd
    /// to the argument offset according to the directive whence
    pub fn sys_lseek(&self, fd: FileDesc, offset: i64, whence: u8) -> SysResult {
        const SEEK_SET: u8 = 0;
        const SEEK_CUR: u8 = 1;
        const SEEK_END: u8 = 2;

        let pos = match whence {
            SEEK_SET => SeekFrom::Start(offset as u64),
            SEEK_END => SeekFrom::End(offset),
            SEEK_CUR => SeekFrom::Current(offset),
            _ => return Err(LxError::EINVAL),
        };
        info!("lseek: fd={:?}, pos={:?}", fd, pos);

        let proc = self.linux_process();
        // Use the FileLike seek so it works on non-`File` fds too — notably the
        // DRM PRIME dma-buf fd, which Mesa's software importer sizes with
        // lseek(SEEK_END). `get_file` would reject it with EBADF.
        let file = proc.get_file_like(fd)?;
        let offset = file.seek(pos)?;
        Ok(offset as usize)
    }

    /// cause the regular file named by path to be truncated to a size of precisely length bytes.
    pub fn sys_truncate(&self, path: UserInPtr<u8>, len: usize) -> SysResult {
        let path = path.as_c_str()?;
        info!("truncate: path={:?}, len={}", path, len);
        let proc = self.linux_process();
        let inode = proc.lookup_inode(path)?;
        let metadata = inode.metadata()?;
        proc.check_access(&metadata, 0o2, true)?;
        inode.resize(len)?;
        Ok(0)
    }

    /// cause the regular file referenced by fd to be truncated to a size of precisely length bytes.
    pub fn sys_ftruncate(&self, fd: FileDesc, len: usize) -> SysResult {
        info!("ftruncate: fd={:?}, len={}", fd, len);
        let proc = self.linux_process();
        let file = proc.get_file(fd)?;
        // The desktop OOM was a single ftruncate growing one RAM-backed file
        // to ~456 MiB (117k live 4 KiB ramfs blocks in one resize). Name any
        // suspicious-sized truncate loudly: which file, how big, and who.
        if len >= 16 * 1024 * 1024 {
            warn!(
                "[ftruncate] BIG: len={} MiB path={} comm={}",
                len >> 20,
                file.path(),
                self.zircon_process().name(),
            );
        }
        file.set_len(len as u64)?;
        // Leak telemetry: the desktop OOM is unfreed memfd content, and
        // ftruncate is the call that sizes memfds — every 64th call, log the
        // live-memfd census (count, sizes, strong refs) so the growth and its
        // holder are visible in real time without waiting for the OOM.
        {
            use core::sync::atomic::{AtomicUsize, Ordering};
            static FTRUNCATE_N: AtomicUsize = AtomicUsize::new(0);
            if FTRUNCATE_N
                .fetch_add(1, Ordering::Relaxed)
                .is_multiple_of(64)
            {
                linux_object::fs::memfd_dump_live(8);
            }
        }
        Ok(0)
    }

    /// Announce an intention to access file data in a specific pattern
    /// (`posix_fadvise`). The hint is purely advisory, so we validate the
    /// descriptor and otherwise treat it as a no-op returning success. This
    /// silences the spurious `unknown syscall: FADVISE64` errors emitted by
    /// tools such as `e2fsck`.
    pub fn sys_fadvise64(
        &self,
        fd: FileDesc,
        offset: usize,
        len: usize,
        advice: usize,
    ) -> SysResult {
        info!(
            "fadvise64: fd={:?}, offset={}, len={}, advice={}",
            fd, offset, len, advice
        );
        // Honour Linux's EBADF for an invalid descriptor; ignore the hint itself.
        let _ = self.linux_process().get_file_like(fd)?;
        Ok(0)
    }

    /// Manipulate the allocated disk space for the file referenced by `fd`
    /// (`fallocate`). We support the default mode by growing a regular file so
    /// that `offset + len` bytes are backed; every other mode (and any non
    /// regular file such as a block device) is treated as a successful no-op.
    /// That is enough for `resize2fs`/`e2fsck`, which only rely on the size
    /// effect, and avoids the `unknown syscall: FALLOCATE` errors.
    pub fn sys_fallocate(&self, fd: FileDesc, mode: usize, offset: usize, len: usize) -> SysResult {
        info!(
            "fallocate: fd={:?}, mode={:#x}, offset={}, len={}",
            fd, mode, offset, len
        );
        let file = self.linux_process().get_file(fd)?;
        // Only the plain allocate mode (mode == 0) implies the file may need to
        // grow. KEEP_SIZE, the hole-punch/zero-range variants, and any request
        // against a non-regular file (e.g. a block device) must leave the size
        // untouched, so they fall through to a successful no-op.
        if mode == 0 {
            let meta = file.metadata()?;
            if meta.type_ == linux_object::fs::vfs::FileType::File {
                let end = offset.checked_add(len).ok_or(LxError::EINVAL)?;
                if end > meta.size {
                    file.set_len(end as u64)?;
                }
            }
        }
        Ok(0)
    }

    /// copies data between one file descriptor and another.
    pub async fn sys_sendfile(
        &self,
        out_fd: FileDesc,
        in_fd: FileDesc,
        offset_ptr: UserInOutPtr<u64>,
        count: usize,
    ) -> SysResult {
        self.sys_copy_file_range(in_fd, offset_ptr, out_fd, 0.into(), count, 0)
            .await
    }

    /// copies data between one file descriptor and anothe, read from specified offset and write new offset back
    pub async fn sys_copy_file_range(
        &self,
        in_fd: FileDesc,
        mut in_offset: UserInOutPtr<u64>,
        out_fd: FileDesc,
        mut out_offset: UserInOutPtr<u64>,
        count: usize,
        flags: usize,
    ) -> SysResult {
        info!(
            "copy_file_range: in={:?}, out={:?}, in_offset={:?}, out_offset={:?}, count={}, flags={}",
            in_fd, out_fd, in_offset, out_offset, count, flags
        );
        let proc = self.linux_process();
        let in_file = proc.get_file(in_fd)?;
        let out_file = proc.get_file(out_fd)?;
        let mut buffer = alloc::vec![0u8; 1024];

        // for in_offset and out_offset
        // null means update file offset
        // non-null means update {in,out}_offset instead

        let mut read_offset = if !in_offset.is_null() {
            in_offset.read()?
        } else {
            in_file.seek(SeekFrom::Current(0))?
        };

        let orig_out_file_offset = out_file.seek(SeekFrom::Current(0))?;
        let write_offset = if !out_offset.is_null() {
            let offset = out_offset.read()?;
            out_file.seek(SeekFrom::Start(offset))?
        } else {
            0
        };

        // read from specified offset and write new offset back
        let mut bytes_read = 0;
        let mut total_written = 0;
        while bytes_read < count {
            let len = buffer.len().min(count - bytes_read);
            let read_len = in_file.read_at(read_offset, &mut buffer[..len]).await?;
            if read_len == 0 {
                break;
            }
            bytes_read += read_len;
            read_offset += read_len as u64;

            let mut bytes_written = 0;
            let mut rlen = read_len;
            while bytes_written < read_len {
                let write_len = out_file.write(&buffer[bytes_written..(bytes_written + rlen)])?;
                if write_len == 0 {
                    info!(
                        "copy_file_range:END_ERR in={:?}, out={:?}, in_offset={:?}, out_offset={:?}, count={} = bytes_read {}, bytes_written {}, write_len {}",
                        in_fd, out_fd, in_offset, out_offset, count, bytes_read, bytes_written, write_len
                    );
                    return Err(LxError::EBADF);
                }
                bytes_written += write_len;
                rlen -= write_len;
            }
            total_written += bytes_written;
        }

        if !in_offset.is_null() {
            in_offset.write(read_offset)?;
        } else {
            in_file.seek(SeekFrom::Current(bytes_read as i64))?;
        }
        out_offset.write_if_not_null(write_offset + total_written as u64)?;
        if !out_offset.is_null() {
            out_file.seek(SeekFrom::Start(orig_out_file_offset))?;
        }
        Ok(total_written)
    }

    /// causes all buffered modifications to file metadata and data to be written to the underlying file systems.
    pub fn sys_sync(&self) -> SysResult {
        info!("sync:");
        let proc = self.linux_process();
        proc.root_inode().fs().sync()?;
        Ok(0)
    }

    /// transfers ("flushes") all modified in-core data of (i.e., modified buffer cache pages for) the file
    /// referred to by the file descriptor fd to the disk device
    pub fn sys_fsync(&self, fd: FileDesc) -> SysResult {
        info!("fsync: fd={:?}", fd);
        let proc = self.linux_process();
        proc.get_file(fd)?.sync_all()?;
        Ok(0)
    }

    /// is similar to fsync(), but does not flush modified metadata unless that metadata is needed
    pub fn sys_fdatasync(&self, fd: FileDesc) -> SysResult {
        info!("fdatasync: fd={:?}", fd);
        let proc = self.linux_process();
        proc.get_file(fd)?.sync_data()?;
        Ok(0)
    }

    /// Commit filesystem caches of the filesystem containing `fd` to disk
    /// (see syncfs(2)). Unlike `sync`, only that one filesystem is flushed.
    pub fn sys_syncfs(&self, fd: FileDesc) -> SysResult {
        info!("syncfs: fd={:?}", fd);
        let proc = self.linux_process();
        proc.get_file(fd)?.inode().fs().sync()?;
        Ok(0)
    }

    /// initiate file readahead into the page cache
    /// (see [linux man readahead(2)](https://www.man7.org/linux/man-pages/man2/readahead.2.html)).
    ///
    /// Purely an optimisation hint on Linux. File reads here are synchronous
    /// (no background page-cache fill to kick off), so after validating that
    /// `fd` names a real seekable file — the errors the man page requires —
    /// there is nothing useful left to start, and success is the honest reply.
    pub fn sys_readahead(&self, fd: FileDesc, offset: u64, count: usize) -> SysResult {
        info!("readahead: fd={:?}, offset={}, count={}", fd, offset, count);
        let proc = self.linux_process();
        proc.get_file(fd)?;
        Ok(0)
    }

    /// sync a file segment with disk
    /// (see [linux man sync_file_range(2)](https://www.man7.org/linux/man-pages/man2/sync_file_range.2.html)).
    ///
    /// Flag validation per the man page; the actual sync is delegated to the
    /// file's `sync_data` when a write-back is requested — syncing more than
    /// the asked-for range is explicitly permitted behaviour, and it is the
    /// only granularity the filesystems here offer.
    pub fn sys_sync_file_range(
        &self,
        fd: FileDesc,
        offset: u64,
        nbytes: u64,
        flags: usize,
    ) -> SysResult {
        const SYNC_FILE_RANGE_WAIT_BEFORE: usize = 1;
        const SYNC_FILE_RANGE_WRITE: usize = 2;
        const SYNC_FILE_RANGE_WAIT_AFTER: usize = 4;
        info!(
            "sync_file_range: fd={:?}, offset={}, nbytes={}, flags={:#x}",
            fd, offset, nbytes, flags
        );
        if flags
            & !(SYNC_FILE_RANGE_WAIT_BEFORE | SYNC_FILE_RANGE_WRITE | SYNC_FILE_RANGE_WAIT_AFTER)
            != 0
        {
            return Err(LxError::EINVAL);
        }
        let proc = self.linux_process();
        let file = proc.get_file(fd)?;
        if flags != 0 {
            file.sync_data()?;
        }
        Ok(0)
    }

    /// DRM ioctls that must install / look up a process fd (PRIME dma-buf
    /// export-import, and CREATE_LEASE) — the inode-level DRM `io_control`
    /// cannot touch the fd table. Returns `Ok(Some(0))` if handled, `Ok(None)`
    /// if `fd` is not a DRM device (fall through to the normal ioctl path).
    fn sys_drm_prime(
        &self,
        file_like: &alloc::sync::Arc<dyn linux_object::fs::FileLike>,
        request: usize,
        arg1: usize,
    ) -> Result<Option<usize>, LxError> {
        use linux_object::fs::devfs::drm;
        use linux_object::fs::DmaBuf;

        const PRIME_HANDLE_TO_FD: usize = 0xC00C_642E; // DRM_IOWR(0x2e, drm_prime_handle)
        const PRIME_FD_TO_HANDLE: usize = 0xC00C_642D; // DRM_IOWR(0x2d, drm_prime_handle)
        const MODE_CREATE_LEASE: usize = 0xC018_64C6;

        // struct drm_prime_handle { __u32 handle; __u32 flags; __s32 fd; }
        #[repr(C)]
        #[derive(Clone, Copy, Default)]
        struct DrmPrimeHandle {
            handle: u32,
            flags: u32,
            fd: i32,
        }

        // struct drm_mode_create_lease {
        //   __u64 object_ids; __u32 object_count; __u32 flags;
        //   __u32 lessee_id;  __s32 fd; }
        #[repr(C)]
        #[derive(Clone, Copy, Default)]
        struct DrmModeCreateLease {
            object_ids: u64,
            object_count: u32,
            flags: u32,
            lessee_id: u32,
            fd: i32,
        }

        // These ioctl numbers are DRM-specific (libdrm only issues them on a DRM
        // fd), and each operation errors gracefully on a wrong fd, so handle by
        // request number alone — no fragile fd-type detection.
        let proc = self.linux_process();
        match request {
            // EXPORT (HANDLE_TO_FD) and IMPORT (FD_TO_HANDLE) share one arm and
            // are told apart by STRUCT CONTENT, not the ioctl number. On real
            // hardware the number-based dispatch mis-routed 0xc00c642d (export)
            // into the import path — verified impossible in the source yet
            // reproducible on the target — so the number is no longer trusted to
            // pick the operation. libdrm's drmPrimeHandleToFD presets the output
            // `fd` field to -1 and passes a real GEM `handle`; drmPrimeFDToHandle
            // passes a real `fd` (>=0) and a zero output `handle`. Thus `fd < 0`
            // is an unambiguous, dispatch-independent marker of an export.
            PRIME_HANDLE_TO_FD | PRIME_FD_TO_HANDLE => {
                let mut ptr = UserInOutPtr::<DrmPrimeHandle>::from(arg1);
                let mut h = match ptr.read() {
                    Ok(h) => h,
                    Err(e) => {
                        warn!("[drm] PRIME read(args @ {:#x}) EFAULT: {:?}", arg1, e);
                        return Err(e.into());
                    }
                };
                let is_export = h.fd < 0;
                if is_export {
                    // handle -> new dma-buf fd
                    let (phys, size, vmo) = match drm::export_handle(h.handle) {
                        Some(v) => v,
                        None => {
                            warn!(
                                "[drm] PRIME export EINVAL: handle={} not in GEM table",
                                h.handle
                            );
                            return Err(LxError::EINVAL);
                        }
                    };
                    let dmabuf = DmaBuf::new(phys, size, vmo);
                    let new_fd = match proc.add_file(dmabuf) {
                        Ok(fd) => fd,
                        Err(e) => {
                            warn!("[drm] PRIME export add_file {:?}", e);
                            return Err(e);
                        }
                    };
                    h.fd = i32::from(new_fd);
                    // debug-only: export is a HOT path. wlroots re-exports both
                    // swapchain buffers on every recreate/present cycle, so at
                    // error! this floods the graphical console (~8/s) and buries
                    // the real failure in labwc.log -- exactly the console-flood
                    // pattern to avoid. The diagnostic job (compare exported phys
                    // vs the import-side reverse lookup) is done; keep it at debug.
                    log::debug!(
                        "[drm] PRIME export handle={:#x} -> phys={:#x} size={} fd={}",
                        h.handle, phys, size, h.fd
                    );
                    if let Err(e) = ptr.write(h) {
                        warn!("[drm] PRIME export write-back EFAULT: {:?}", e);
                        return Err(e.into());
                    }
                    Ok(Some(0))
                } else {
                    // fd -> new GEM handle
                    let target = match proc.get_file_like(FileDesc::from(h.fd as usize)) {
                        Ok(t) => t,
                        Err(e) => {
                            warn!("[drm] PRIME import EBADF: fd={} not in fd table", h.fd);
                            return Err(e);
                        }
                    };
                    let dmabuf = target.downcast_ref::<DmaBuf>().ok_or(LxError::EINVAL)?;
                    // Self-import first: if this dma-buf was exported from a
                    // nouveau-uAPI GEM object, hand back the ORIGINAL nouveau
                    // handle -- real Linux PRIME semantics (importing your own
                    // export resolves to the existing GEM object). This is
                    // what the compositor does for every swapchain buffer:
                    // gbm/NVK allocates (GEM_NEW), exports the fd, and EGL
                    // imports it again on the same device. A fresh GENERIC
                    // handle over the same memory (the old behaviour) passes
                    // the generic ioctls and then fails every driver-private
                    // one -- nouveau GEM_INFO gave ENOENT, NVK's dma-buf
                    // import died, and the desktop fell over at
                    // "createImageFromDmaBufs failed" / zink "couldn't
                    // allocate memory".
                    let (handle_id, kind) = match drm::nouveau_handle_for_phys(dmabuf.phys_addr) {
                        Some(nouveau_handle) => {
                            // Take a PRIME reference: the importer will GEM_CLOSE
                            // this handle when done, but the exporter (wlroots)
                            // still owns the same nouveau handle. Without this
                            // bump the importer's close frees the buffer out from
                            // under the exporter, the next self-import misses, and
                            // NVK falls back to a generic handle that GEM_INFO
                            // ENOENTs -> zink "couldn't allocate memory heap=0".
                            let n = drm::nouveau_gem_add_ref(nouveau_handle);
                            // debug-only: this is the SUCCESS path and it is hot
                            // (every swapchain buffer import). Stay quiet so a
                            // working stack does not flood the console; the failure
                            // path below is the one that logs loudly.
                            log::debug!(
                                "[drm] PRIME self-import ref++ handle={:#x} -> refcount={:?}",
                                nouveau_handle, n
                            );
                            // ...except the first few per boot, at error! (the
                            // rig runs LOG=error): whether the COMPOSITOR ever
                            // imports a CLIENT's dma-buf is the missing half of
                            // the vkcube/eglgears present-hang picture. The pid
                            // tells the two apart -- labwc importing a phys that
                            // a client pid exported means the Wayland dmabuf
                            // dance reached the compositor at all.
                            {
                                use core::sync::atomic::{AtomicU32, Ordering};
                                static FIRST_IMPORTS: AtomicU32 = AtomicU32::new(0);
                                let k = FIRST_IMPORTS.fetch_add(1, Ordering::Relaxed);
                                if k < 8 {
                                    log::error!(
                                        "[drm] PRIME import pid={} fd={} phys={:#x} size={} -> nouveau handle={:#x} (import {}/8 this boot; informational, not an error)",
                                        self.zircon_process().id(), h.fd,
                                        dmabuf.phys_addr, dmabuf.size, nouveau_handle, k + 1
                                    );
                                }
                            }
                            (nouveau_handle, "self-import(nouveau)")
                        }
                        None => (
                            drm::import_dmabuf(dmabuf.phys_addr, dmabuf.size, dmabuf.vmo()),
                            "generic",
                        ),
                    };
                    // Loud ONLY on the failure path: "generic" means the
                    // self-import reverse lookup MISSED (the dma-buf's phys is not
                    // registered as a nouveau GEM object), so NVK's driver-private
                    // GEM_INFO will ENOENT the fresh handle -> zink "couldn't
                    // allocate memory heap=0" / createImageFromDmaBufs failed. The
                    // success path ("self-import(nouveau)") is hot and stays at
                    // debug so a working swapchain loop doesn't flood the console.
                    if kind == "generic" {
                        log::error!(
                            "[drm] PRIME import fd={} phys={:#x} size={} -> generic handle={:#x} (SELF-IMPORT MISS -> GEM_INFO will ENOENT)",
                            h.fd, dmabuf.phys_addr, dmabuf.size, handle_id
                        );
                    } else {
                        log::debug!(
                            "[drm] PRIME import fd={} phys={:#x} size={} -> {} handle={:#x}",
                            h.fd, dmabuf.phys_addr, dmabuf.size, kind, handle_id
                        );
                    }
                    h.handle = handle_id;
                    ptr.write(h)?;
                    Ok(Some(0))
                }
            }
            MODE_CREATE_LEASE => {
                // An empty lease is just a fresh fd to the same DRM device: our
                // GEM table is global, so per-fd handle ref-counting (the reason
                // wlroots' dumb allocator leases) is unnecessary. Hand back a dup
                // of this fd as the lease.
                let mut ptr = UserInOutPtr::<DrmModeCreateLease>::from(arg1);
                let mut l = ptr.read()?;
                let lease = file_like.dup();
                let new_fd = proc.add_file(lease)?;
                l.lessee_id = 1;
                l.fd = i32::from(new_fd);
                ptr.write(l)?;
                Ok(Some(0))
            }
            _ => Ok(None),
        }
    }

    /// `SYNCOBJ_HANDLE_TO_FD` / `SYNCOBJ_FD_TO_HANDLE` (`drm.h`, core DRM,
    /// not driver-private) -- like PRIME dma-buf export/import above, these
    /// need process fd table access the DRM inode's `io_control` doesn't
    /// have. See [`linux_object::fs::SyncobjHandle`]'s module doc for what
    /// "export" means here: the syncobj table is a single global handle
    /// space (not per-process), so this just carries the already-globally-
    /// valid handle number across the fd boundary -- it doesn't move or
    /// copy any state, and closing the fd does NOT destroy the syncobj.
    /// Gated on the same `nvidia.nouveau_uapi` opt-in as the rest of the
    /// syncobj ioctls (`drm_scheme.rs`).
    fn sys_drm_syncobj_fd(&self, request: usize, arg1: usize) -> Result<Option<usize>, LxError> {
        use linux_object::fs::SyncobjHandle;

        const SYNCOBJ_HANDLE_TO_FD: usize = 0xC010_64C1; // DRM_IOWR(0xc1, drm_syncobj_handle)
        const SYNCOBJ_FD_TO_HANDLE: usize = 0xC010_64C2; // DRM_IOWR(0xc2, drm_syncobj_handle)
        const IMPORT_SYNC_FILE: u32 = 1 << 0;

        if request != SYNCOBJ_HANDLE_TO_FD && request != SYNCOBJ_FD_TO_HANDLE {
            return Ok(None);
        }
        if !kernel_hal::drivers::nouveau_uapi_enabled() {
            return Ok(None);
        }

        // struct drm_syncobj_handle { __u32 handle; __u32 flags; __s32 fd; __u32 pad; }
        #[repr(C)]
        #[derive(Clone, Copy, Default)]
        struct DrmSyncobjHandle {
            handle: u32,
            flags: u32,
            fd: i32,
            pad: u32,
        }

        let proc = self.linux_process();
        let mut ptr = UserInOutPtr::<DrmSyncobjHandle>::from(arg1);
        let mut h = match ptr.read() {
            Ok(h) => h,
            Err(e) => {
                warn!("[drm] SYNCOBJ read(args @ {:#x}) EFAULT: {:?}", arg1, e);
                return Err(e.into());
            }
        };
        if h.flags & IMPORT_SYNC_FILE != 0 {
            // The `_FLAGS_IMPORT_SYNC_FILE` variant interoperates with a
            // POSIX `sync_file` fd (a different kernel object entirely,
            // from a different subsystem) instead of one of these
            // handle-carrying fds. Eclipse has no `sync_file` abstraction
            // to interoperate with -- refuse rather than silently treat a
            // `sync_file` fd as if it were one of ours.
            warn!("[drm] SYNCOBJ_{{HANDLE_TO_FD,FD_TO_HANDLE}}_FLAGS_IMPORT_SYNC_FILE not supported");
            return Err(LxError::EOPNOTSUPP);
        }
        if request == SYNCOBJ_HANDLE_TO_FD {
            if kernel_hal::drivers::scheme::syncobj::query(h.handle).is_none() {
                warn!(
                    "[drm] SYNCOBJ_HANDLE_TO_FD EINVAL: handle={} not a live syncobj",
                    h.handle
                );
                return Err(LxError::EINVAL);
            }
            let file = SyncobjHandle::new(h.handle);
            let new_fd = match proc.add_file(file) {
                Ok(fd) => fd,
                Err(e) => {
                    warn!("[drm] SYNCOBJ_HANDLE_TO_FD add_file {:?}", e);
                    return Err(e);
                }
            };
            h.fd = i32::from(new_fd);
            if let Err(e) = ptr.write(h) {
                warn!("[drm] SYNCOBJ_HANDLE_TO_FD write-back EFAULT: {:?}", e);
                return Err(e.into());
            }
            Ok(Some(0))
        } else {
            let target = match proc.get_file_like(FileDesc::from(h.fd as usize)) {
                Ok(t) => t,
                Err(e) => {
                    warn!(
                        "[drm] SYNCOBJ_FD_TO_HANDLE EBADF: fd={} not in fd table",
                        h.fd
                    );
                    return Err(e);
                }
            };
            let syncobj = target
                .downcast_ref::<SyncobjHandle>()
                .ok_or(LxError::EINVAL)?;
            h.handle = syncobj.handle;
            ptr.write(h)?;
            Ok(Some(0))
        }
    }

    /// Set parameters of device files.
    pub fn sys_ioctl(
        &self,
        fd: FileDesc,
        request: usize,
        arg1: usize,
        arg2: usize,
        arg3: usize,
    ) -> SysResult {
        info!(
            "ioctl: fd={:?}, request={:#x}, args=[{:#x}, {:#x}, {:#x}]",
            fd, request, arg1, arg2, arg3
        );
        let proc = self.linux_process();
        let file_like = match proc.get_file_like(fd) {
            Ok(f) => f,
            Err(e) => {
                // A DRM ioctl ('d' = 0x64 family) on an fd that is NOT in the
                // fd table would otherwise fail EBADF with zero trace — make it
                // visible: this is how a stale/ghost fd in userspace shows up.
                if (request as u32 >> 8) & 0xff == 0x64 {
                    error!(
                        "[drm] pid={} ioctl {:#x}: fd {:?} NOT in fd table -> {:?}",
                        self.zircon_process().id(),
                        request as u32,
                        fd,
                        e
                    );
                }
                return Err(e);
            }
        };
        // File ioctls served at the VFS layer, como en Linux (fs/ioctl.c
        // `do_vfs_ioctl`): they apply to EVERY fd kind — pipes, sockets,
        // files, device nodes — so the per-inode handlers never need to know
        // them. Rust's std is the load-bearing caller: `Command::output()`
        // sets both child report pipes nonblocking via `ioctl(FIONBIO)`
        // (sys/pal/unix/fd.rs `set_nonblocking`), and `TcpStream/UdpSocket::
        // set_nonblocking` do the same on sockets. With no VFS handling,
        // FIONBIO on a pipe fell through to the inode default -> ENOSYS ->
        // the ENOTTY normalization below -> std's `read_output` propagated
        // "Not a tty" into the `res.unwrap()` inside `Command::output()`
        // (library/std/src/sys/process/mod.rs) -> SIGABRT. That was
        // lunarbar's respawn crash loop: its volume probe runs
        // `wpctl`/`amixer` through `Command::output()` on every refresh
        // (tools/lunarbar/src/sysinfo.rs).
        //
        // Exact small values, never sign-extended (bit 31 clear), so there is
        // no collision with the DRM 0xC0xx_64xx family handled below.
        const FIONBIO: usize = 0x5421;
        const FIOCLEX: usize = 0x5451;
        const FIONCLEX: usize = 0x5450;
        match request {
            FIONBIO => {
                // The argument is a pointer to int: nonzero = O_NONBLOCK on.
                let on: UserInPtr<i32> = arg1.into();
                let on = on.read()? != 0;
                let mut flags = file_like.flags();
                flags.set(OpenFlags::NON_BLOCK, on);
                file_like.set_flags(flags)?;
                return Ok(0);
            }
            // Close-on-exec lives in the per-process fd table, exactly like
            // fcntl F_SETFD — never in the shared File object.
            FIOCLEX => {
                proc.set_fd_cloexec(fd, true)?;
                return Ok(0);
            }
            FIONCLEX => {
                proc.set_fd_cloexec(fd, false)?;
                return Ok(0);
            }
            _ => {}
        }
        // DRM PRIME (dma-buf) export/import and CREATE_LEASE — need process fd
        // access, so they are handled here rather than in the DRM inode's
        // io_control.
        //
        // libdrm defines these request numbers via `_IOWR(...)`, whose value
        // has bit 31 set (direction `_IOC_READ|_IOC_WRITE` == 0xC0...) and is
        // typed `int`. Passed to the variadic `ioctl(int, unsigned long, ...)`
        // it sign-extends to 64 bits — e.g. CREATE_LEASE arrives as
        // 0xFFFF_FFFF_C018_64C6, not 0xC018_64C6. The inode-level `io_control`
        // path never saw this because it takes `cmd: u32` (truncated), but here
        // `request` is the full 64-bit value, so mask to 32 bits before
        // matching. Without this the dispatch misses and the ioctl falls
        // through to ENOTTY ("Not a tty").
        let cmd = request as u32 as usize;
        if cmd == 0xC00C_642D || cmd == 0xC00C_642E || cmd == 0xC018_64C6 {
            // `sys_drm_prime` logs only on genuine failures; the wrapper stays
            // silent on the hot path. Ok(None) means "not a PRIME request after
            // all"; fall through to the inode `io_control`.
            match self.sys_drm_prime(&file_like, cmd, arg1) {
                Ok(Some(ret)) => return Ok(ret),
                Ok(None) => {}
                Err(e) => return Err(e),
            }
        }
        // SYNCOBJ_HANDLE_TO_FD / SYNCOBJ_FD_TO_HANDLE — same fd-table-access
        // reasoning and sign-extension caveat as PRIME above.
        if cmd == 0xC010_64C1 || cmd == 0xC010_64C2 {
            match self.sys_drm_syncobj_fd(cmd, arg1) {
                Ok(Some(ret)) => return Ok(ret),
                Ok(None) => {}
                Err(e) => return Err(e),
            }
        }
        // `TIOCSCTTY` on a pts: adopt the pty as the controlling terminal.
        // Linux sets the tty's foreground process group to the caller's pgrp
        // (`tty_jobctrl.c`), which is what makes a `login_tty()`'d shell's very
        // FIRST `tcgetpgrp()` see itself as the foreground job and go straight
        // to the prompt instead of looping in busybox ash's
        // `killpg(0, SIGTTIN)` background wait. The pty inode's own ioctl
        // handler cannot do this — it has no process context — so seed it here.
        const TIOCSCTTY: usize = 0x540E;
        if cmd == TIOCSCTTY {
            if let Some(file) = file_like.downcast_ref::<linux_object::fs::File>() {
                let inode = file.inode();
                if let Some(slave) = inode
                    .as_any_ref()
                    .downcast_ref::<linux_object::fs::pty::PtySlave>()
                {
                    if let Ok(pgid) =
                        linux_object::process::get_process_pgid(self.zircon_process().id())
                    {
                        slave.set_fg_pgrp(pgid as i32);
                    }
                    return Ok(0);
                }
            }
        }
        // `TIOCGWINSZ` (get terminal window size).
        const TIOCGWINSZ: usize = 0x5413;
        let ret = match file_like.ioctl(request, arg1, arg2, arg3) {
            // Some programs insist on a valid window size and keep retrying
            // `TIOCGWINSZ` when it fails on a char device that is not a full
            // tty backend (e.g. DRM/fb). Only synthesize a size for char
            // devices that are not input nodes — pipes/sockets/regular files
            // must get `ENOTTY` so `isatty()` does not lie.
            //
            // Input device nodes (`/dev/input/mice`, `event*`) are excluded:
            // faking a window size there makes musl's `isatty()` (a TIOCGWINSZ
            // probe) report a tty, and kdrive/TinyX then treats the mouse as a
            // serial port and loops over serial mouse protocols.
            Err(LxError::ENOSYS) | Err(LxError::ENOTTY) | Err(LxError::EINVAL)
                if request == TIOCGWINSZ
                    && arg1 != 0
                    && file_like.is_char_device()
                    && !file_like.is_input_device() =>
            {
                let mut ws = kernel_hal::console::console_win_size();
                if ws.ws_col == 0 {
                    ws.ws_col = 80;
                }
                if ws.ws_row == 0 {
                    ws.ws_row = 25;
                }
                let mut ptr: UserOutPtr<kernel_hal::console::ConsoleWinSize> = arg1.into();
                ptr.write(ws)?;
                Ok(0)
            }
            // A tty-class request (TCGETS..TIOCGPTPEER, the 0x54xx block) on
            // something that is not a char device answers ENOTTY on Linux —
            // the file's handler simply does not know the request. Some of our
            // inode impls say EINVAL instead (sfs maps every unknown cmd to
            // `FsError::IOCTLError` -> EINVAL), which musl's `isatty()`
            // tolerates but errno-checking callers do not: that was the
            // `[einval-hunt] IOCTL a1=0x5413 (TIOCGWINSZ) on fd 2 -> EINVAL`
            // hit against a service's log-file stderr. The VFS file ioctls
            // sharing the block (TIOCOUTQ 0x5411 aside: FIONREAD 0x541B,
            // FIONBIO 0x5421, FIONCLEX/FIOCLEX 0x5450/1, FIOASYNC 0x5452,
            // FIOQSIZE 0x545E) are excluded — they are legal on pipes and
            // sockets and answered elsewhere.
            Err(LxError::EINVAL)
                if (0x5400..0x5460).contains(&request)
                    && !matches!(request, 0x541B | 0x5421 | 0x5450 | 0x5451 | 0x5452 | 0x545E)
                    && !file_like.is_char_device() =>
            {
                Err(LxError::ENOTTY)
            }
            // An unhandled ioctl maps to `ENOSYS` ("function not implemented")
            // via the generic FsError conversion, but the POSIX/Linux convention
            // for an ioctl that does not apply to a device is `ENOTTY`
            // ("inappropriate ioctl for device"). Returning `ENOSYS` makes some
            // programs treat it as fatal or retry in a loop, so normalise it.
            Err(LxError::ENOSYS) => Err(LxError::ENOTTY),
            other => other,
        };
        // Surface a genuinely unhandled/failed ioctl on the console (and serial),
        // the same way an invalid syscall number is — but throttle a program
        // busy-looping on the same failing request so it can't flood the console
        // thousands of times a second: log the first occurrence, then only every
        // 4096th repeat. `TIOCGWINSZ` returning `ENOTTY` is the normal "not a
        // terminal" answer (musl's `isatty()` probes devices this way), so it is
        // not logged at all.
        if let Err(e) = &ret {
            if request != TIOCGWINSZ {
                use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
                static LAST_REQ: AtomicU64 = AtomicU64::new(u64::MAX);
                static REPEATS: AtomicUsize = AtomicUsize::new(0);
                let n = if LAST_REQ.swap(request as u64, Ordering::Relaxed) == request as u64 {
                    REPEATS.fetch_add(1, Ordering::Relaxed) + 1
                } else {
                    REPEATS.store(0, Ordering::Relaxed);
                    0
                };
                if n == 0 || n % 4096 == 0 {
                    // warn, not error: many programs probe optional ioctls and
                    // handle the failure themselves (EOPNOTSUPP/ENOTTY is a
                    // legitimate answer). Boot with LOG=warn to see these when
                    // chasing a missing ioctl.
                    warn!(
                        "ioctl fd={:?} request={:#x} -> ERR {:?} (unhandled/failed){}",
                        fd,
                        request,
                        e,
                        if n > 0 { " [repeating, throttled]" } else { "" }
                    );
                }
            }
        }
        ret
    }

    /// Manipulate a file descriptor.
    /// - cmd – cmd flag
    /// - arg – additional parameters based on cmd
    pub async fn sys_fcntl(&self, fd: FileDesc, cmd: usize, arg: usize) -> SysResult {
        info!("fcntl: fd={:?}, cmd={}, arg={}", fd, cmd, arg);
        // memfd file seals (`F_LINUX_SPECIFIC_BASE + 9/10`). We don't enforce
        // seals — there is a single trusted address space — but Wayland/wlroots
        // add `F_SEAL_SHRINK` to keymap memfds and abort if the call fails, so
        // accept additions as a no-op and report "no seals set".
        const F_ADD_SEALS: usize = 1033;
        const F_GET_SEALS: usize = 1034;
        const F_SETPIPE_SZ: usize = 1031;
        const F_GETPIPE_SZ: usize = 1032;
        // POSIX record locks, classic and open-file-description flavours.
        const F_GETLK: usize = 5;
        const F_SETLK: usize = 6;
        const F_SETLKW: usize = 7;
        const F_OFD_GETLK: usize = 36;
        const F_OFD_SETLK: usize = 37;
        const F_OFD_SETLKW: usize = 38;
        let proc = self.linux_process();
        let file_like = proc.get_file_like(fd)?;
        if cmd == F_ADD_SEALS || cmd == F_GET_SEALS {
            return Ok(0);
        }
        if matches!(
            cmd,
            F_GETLK | F_SETLK | F_SETLKW | F_OFD_GETLK | F_OFD_SETLK | F_OFD_SETLKW
        ) {
            let get = cmd == F_GETLK || cmd == F_OFD_GETLK;
            let wait = cmd == F_SETLKW || cmd == F_OFD_SETLKW;
            return self.fcntl_record_lock(&file_like, get, wait, arg).await;
        }
        // Pipe capacity (fcntl(2), F_SETPIPE_SZ/F_GETPIPE_SZ): valid only on
        // pipe fds — Linux answers EBADF for other fd kinds.
        if cmd == F_SETPIPE_SZ || cmd == F_GETPIPE_SZ {
            let file = file_like.downcast_ref::<File>().ok_or(LxError::EBADF)?;
            let inode = file.inode();
            let pipe = inode.downcast_ref::<Pipe>().ok_or(LxError::EBADF)?;
            return if cmd == F_GETPIPE_SZ {
                Ok(pipe.capacity())
            } else {
                let size = pipe_size_round(arg)?;
                pipe.set_capacity(size);
                // Linux returns the actual (rounded) capacity, not 0.
                Ok(size)
            };
        }
        if let Ok(cmd) = FcntlCmd::try_from(cmd) {
            match cmd {
                // FD_CLOEXEC is per-DESCRIPTOR state, kept in the process's fd
                // table (see LinuxProcessInner::cloexec_fds) — never in the
                // File object, which fork shares between processes.
                FcntlCmd::GETFD => Ok(proc.fd_cloexec(fd)? as usize),
                FcntlCmd::SETFD => {
                    proc.set_fd_cloexec(fd, (arg & 1) != 0)?;
                    Ok(0)
                }
                FcntlCmd::GETFL => Ok(file_like.flags().bits()),
                FcntlCmd::SETFL => {
                    file_like.set_flags(OpenFlags::from_bits_truncate(arg))?;
                    Ok(0)
                }
                FcntlCmd::DUPFD | FcntlCmd::DUPFD_CLOEXEC => {
                    let new_fd = proc.get_free_fd_from(arg);
                    // sys_dup2 registers the new fd with CLOEXEC off (POSIX
                    // dup semantics); only the _CLOEXEC variant re-tags it.
                    self.sys_dup2(fd, new_fd)?;
                    if cmd == FcntlCmd::DUPFD_CLOEXEC {
                        proc.set_fd_cloexec(new_fd, true)?;
                    }
                    Ok(new_fd.into())
                }
                _ => Err(LxError::EINVAL),
            }
        } else {
            Err(LxError::EINVAL)
        }
    }

    /// The lock half of `fcntl(2)`: F_GETLK probes, F_SETLK acquires or
    /// releases in one shot, F_SETLKW retries until the range frees up. The
    /// range table itself lives in [`linux_object::fs::record_lock`].
    async fn fcntl_record_lock(
        &self,
        file_like: &Arc<dyn FileLike>,
        get: bool,
        wait: bool,
        arg: usize,
    ) -> SysResult {
        use core::time::Duration;
        use linux_object::fs::record_lock::{self, LockRequest, F_RDLCK, F_UNLCK, F_WRLCK};

        let mut ptr: UserInOutPtr<Flock> = arg.into();
        let mut fl = ptr.read()?;
        // Record locks apply to files; pipes/sockets answer EBADF like Linux.
        let file = file_like.downcast_ref::<File>().ok_or(LxError::EBADF)?;
        let meta = file.metadata()?;
        let key = (meta.dev, meta.inode);

        // Resolve l_whence + l_start (+ negative l_len) to an absolute range.
        let base = match fl.l_whence {
            0 => 0i64,                                         // SEEK_SET
            1 => file_like.seek(SeekFrom::Current(0))? as i64, // SEEK_CUR
            2 => meta.size as i64,                             // SEEK_END
            _ => return Err(LxError::EINVAL),
        };
        let mut start = base + fl.l_start;
        let mut len = fl.l_len;
        if len < 0 {
            // POSIX: negative length means the range BEFORE l_start.
            start += len;
            len = -len;
        }
        if start < 0 {
            return Err(LxError::EINVAL);
        }
        let start = start as u64;
        let end = if len == 0 {
            u64::MAX
        } else {
            start.saturating_add(len as u64)
        };
        let owner = self.zircon_process().id();

        match fl.l_type {
            F_UNLCK if !get => {
                let req = LockRequest {
                    exclusive: false,
                    start,
                    end,
                    owner,
                };
                record_lock::setlk(key, &req, true);
                Ok(0)
            }
            F_RDLCK | F_WRLCK => {
                let req = LockRequest {
                    exclusive: fl.l_type == F_WRLCK,
                    start,
                    end,
                    owner,
                };
                if get {
                    match record_lock::getlk(key, &req) {
                        None => fl.l_type = F_UNLCK,
                        Some(c) => {
                            fl.l_type = c.type_;
                            fl.l_whence = 0;
                            fl.l_start = c.start as i64;
                            fl.l_len = c.len as i64;
                            fl.l_pid = c.pid as i32;
                        }
                    }
                    ptr.write(fl)?;
                    Ok(0)
                } else if wait {
                    // Contended F_SETLKW: retry on a short timer. Lock churn is
                    // rare (package-manager style whole-file locks), so a poll
                    // loop beats wiring a waiter queue through the table.
                    loop {
                        if record_lock::setlk(key, &req, false) {
                            return Ok(0);
                        }
                        kernel_hal::thread::sleep_until(kernel_hal::timer::deadline_after(
                            Duration::from_millis(10),
                        ))
                        .await;
                    }
                } else if record_lock::setlk(key, &req, false) {
                    Ok(0)
                } else {
                    Err(LxError::EAGAIN)
                }
            }
            _ => Err(LxError::EINVAL),
        }
    }

    /// Checks whether the calling process can access the file pathname
    pub fn sys_access(&self, path: UserInPtr<u8>, mode: usize) -> SysResult {
        self.sys_faccessat(FileDesc::CWD, path, mode, 0)
    }

    /// Check user's permissions of a file relative to a directory file descriptor
    pub fn sys_faccessat(
        &self,
        dirfd: FileDesc,
        path: UserInPtr<u8>,
        mode: usize,
        flags: usize,
    ) -> SysResult {
        let path = path.as_c_str()?;
        let flags = AtFlags::from_bits_truncate(flags);
        info!(
            "faccessat: dirfd={:?}, path={:?}, mode={:#o}, flags={:?}",
            dirfd, path, mode, flags
        );
        let proc = self.linux_process();
        let follow = !flags.contains(AtFlags::SYMLINK_NOFOLLOW);
        let inode = proc.lookup_inode_at(dirfd, path, follow)?;
        let metadata = inode.metadata()?;
        let requested = (mode & 0o7) as u16;
        let use_effective = flags.contains(AtFlags::EACCESS);
        proc.check_access(&metadata, requested, use_effective)?;
        Ok(0)
    }

    /// Change file mode by descriptor.
    pub fn sys_fchmod(&self, fd: FileDesc, mode: usize) -> SysResult {
        let proc = self.linux_process();
        let inode = proc.get_file(fd)?.inode();
        let mut metadata = inode.metadata()?;
        proc.chmod_metadata(&mut metadata, mode as u16)?;
        inode.set_metadata(&metadata)?;
        Ok(0)
    }

    /// Change file mode relative to a directory file descriptor.
    pub fn sys_fchmodat(
        &self,
        dirfd: FileDesc,
        path: UserInPtr<u8>,
        mode: usize,
        flags: usize,
    ) -> SysResult {
        let path = path.as_c_str()?;
        let flags = AtFlags::from_bits_truncate(flags);
        let follow = !flags.contains(AtFlags::SYMLINK_NOFOLLOW);
        let proc = self.linux_process();
        let inode = proc.lookup_inode_at(dirfd, path, follow)?;
        let mut metadata = inode.metadata()?;
        proc.chmod_metadata(&mut metadata, mode as u16)?;
        inode.set_metadata(&metadata)?;
        Ok(0)
    }

    /// Change file owner/group by descriptor.
    pub fn sys_fchown(&self, fd: FileDesc, uid: usize, gid: usize) -> SysResult {
        let proc = self.linux_process();
        let inode = proc.get_file(fd)?.inode();
        let mut metadata = inode.metadata()?;
        proc.chown_metadata(&mut metadata, uid as u32, gid as u32)?;
        inode.set_metadata(&metadata)?;
        Ok(0)
    }

    /// Change file owner/group relative to a directory file descriptor.
    pub fn sys_fchownat(
        &self,
        dirfd: FileDesc,
        path: UserInPtr<u8>,
        uid: usize,
        gid: usize,
        flags: usize,
    ) -> SysResult {
        let path = path.as_c_str()?;
        let flags = AtFlags::from_bits_truncate(flags);
        let follow = !flags.contains(AtFlags::SYMLINK_NOFOLLOW);
        let proc = self.linux_process();
        let inode = proc.lookup_inode_at(dirfd, path, follow)?;
        let mut metadata = inode.metadata()?;
        proc.chown_metadata(&mut metadata, uid as u32, gid as u32)?;
        inode.set_metadata(&metadata)?;
        Ok(0)
    }

    /// change file timestamps with nanosecond precision
    pub fn sys_utimensat(
        &mut self,
        dirfd: FileDesc,
        pathname: UserInPtr<u8>,
        times: UserInOutPtr<[TimeSpec; 2]>,
        flags: usize,
    ) -> SysResult {
        info!(
            "utimensat(raw): dirfd: {:?}, pathname: {:?}, times: {:?}, flags: {:#x}",
            dirfd, pathname, times, flags
        );
        const UTIME_NOW: usize = 0x3fffffff;
        const UTIME_OMIT: usize = 0x3ffffffe;
        let proc = self.linux_process();
        let mut times = if times.is_null() {
            let epoch = TimeSpec::now();
            [epoch, epoch]
        } else {
            let times = times.read()?;
            [times[0], times[1]]
        };
        let inode = if pathname.is_null() {
            let fd = dirfd;
            info!("futimens: fd: {:?}, times: {:?}", fd, times);
            proc.get_file(fd)?.inode()
        } else {
            let pathname = pathname.as_c_str()?;
            info!(
                "utimensat: dirfd: {:?}, pathname: {:?}, times: {:?}, flags: {:#x}",
                dirfd, pathname, times, flags
            );
            let follow = if flags == 0 {
                true
            } else if flags == AtFlags::SYMLINK_NOFOLLOW.bits() {
                false
            } else {
                return Err(LxError::EINVAL);
            };
            proc.lookup_inode_at(dirfd, pathname, follow)?
        };
        let mut metadata = inode.metadata()?;
        if times[0].nsec != UTIME_OMIT {
            if times[0].nsec == UTIME_NOW {
                times[0] = TimeSpec::now();
            }
            metadata.atime = rcore_fs::vfs::Timespec {
                sec: times[0].sec as i64,
                nsec: times[0].nsec as i32,
            };
        }
        if times[1].nsec != UTIME_OMIT {
            if times[1].nsec == UTIME_NOW {
                times[1] = TimeSpec::now();
            }
            metadata.mtime = rcore_fs::vfs::Timespec {
                sec: times[1].sec as i64,
                nsec: times[1].nsec as i32,
            };
        }
        inode.set_metadata(&metadata)?;
        Ok(0)
    }

    /// Get filesystem statistics
    /// (see [linux man statfs(2)](https://man7.org/linux/man-pages/man2/statfs.2.html)).
    ///
    /// The `statfs` system call returns information about a mounted filesystem.
    /// `path` is the pathname of **any file** within the mounted filesystem.
    /// `buf` is a pointer to a `StatFs` structure.
    pub fn sys_statfs(&self, path: UserInPtr<u8>, mut buf: UserOutPtr<StatFs>) -> SysResult {
        let path = path.as_c_str()?;
        info!("statfs: path={:?}, buf={:?}", path, buf);

        let inode = self.linux_process().lookup_inode(path)?;
        let info = inode.fs().info();
        buf.write(info.into())?;
        Ok(0)
    }

    /// Get filesystem statistics
    /// (see [linux man statfs(2)](https://man7.org/linux/man-pages/man2/statfs.2.html)).
    ///
    /// The `fstatfs` system call returns information about a mounted filesystem.
    /// `fd` is the descriptor referencing an open file.
    /// `buf` is a pointer to a `StatFs` structure.
    pub fn sys_fstatfs(&self, fd: FileDesc, mut buf: UserOutPtr<StatFs>) -> SysResult {
        info!("statfs: fd={:?}, buf={:?}", fd, buf);

        let info = self.linux_process().get_file(fd)?.inode().fs().info();
        buf.write(info.into())?;
        Ok(0)
    }
}

const F_LINUX_SPECIFIC_BASE: usize = 1024;

/// The file system statistics struct defined in linux
/// (see [linux man statfs(2)](https://man7.org/linux/man-pages/man2/statfs.2.html)).
#[repr(C)]
pub struct StatFs {
    f_type: i64,
    f_bsize: i64,
    f_blocks: u64,
    f_bfree: u64,
    f_bavail: u64,
    f_files: u64,
    f_ffree: u64,
    f_fsid: (i32, i32),
    f_namelen: isize,
    f_frsize: isize,
    f_flags: isize,
    f_spare: [isize; 4],
}

// 保证 `StatFs` 的定义和常见的 linux 一致
static_assertions::const_assert_eq!(120, core::mem::size_of::<StatFs>());

impl From<FsInfo> for StatFs {
    fn from(info: FsInfo) -> Self {
        StatFs {
            // TODO 文件系统的魔数，需要 rcore-fs 提供一个渠道获取
            // 但是这个似乎并没有什么用处，新的 vfs 相关函数都去掉了，也许永远填个常数就好了
            f_type: 0,
            f_bsize: info.bsize as _,
            f_blocks: info.blocks as _,
            f_bfree: info.bfree as _,
            f_bavail: info.bavail as _,
            f_files: info.files as _,
            f_ffree: info.ffree as _,
            // 一个由 OS 决定的号码，用于区分文件系统
            f_fsid: (0, 0),
            f_namelen: info.namemax as _,
            f_frsize: info.frsize as _,
            // TODO 需要先实现挂载
            f_flags: 0,
            f_spare: [0; 4],
        }
    }
}

numeric_enum_macro::numeric_enum! {
    #[repr(usize)]
    #[allow(non_camel_case_types)]
    #[derive(Eq, PartialEq, Debug, Copy, Clone)]
    /// fcntl flags
    pub enum FcntlCmd {
        /// dup
        DUPFD = 0,
        /// get close_on_exec
        GETFD = 1,
        /// set/clear close_on_exec
        SETFD = 2,
        /// get file->f_flags
        GETFL = 3,
        /// set file->f_flags
        SETFL = 4,
        /// Get record locking info.
        GETLK = 5,
        /// Set record locking info (non-blocking).
        SETLK = 6,
        /// Set record locking info (blocking).
        SETLKW = 7,
        /// like F_DUPFD, but additionally set the close-on-exec flag
        DUPFD_CLOEXEC = F_LINUX_SPECIFIC_BASE + 6,
    }
}

/// Tee X-server log/error lines into the dmesg ring (prefixed `XLOG:`) so the
/// reason a graphics server (Xorg) aborts is visible via `dmesg`, even when its
/// own logfile is unreachable. Scans a small prefix of each write for the
/// markers Xorg uses for warnings, errors and fatals.
fn tee_x_diag(buf: &[u8]) {
    let scan = &buf[..buf.len().min(1024)];
    let has = |needle: &[u8]| scan.windows(needle.len()).any(|w| w == needle);
    // Xorg's own log markers …
    let x_marker =
        has(b"(EE)") || has(b"(WW)") || has(b"Fatal") || has(b"no screens") || has(b"(II) ");
    // … plus the messages the *dynamic linker* prints to stderr when a program
    // dies before it ever reaches main(). Xorg pulls in far more shared
    // libraries than a typical CLI app, so a single missing `.so` or unresolved
    // symbol makes musl's ld abort with one of these — and that path produces
    // no Xorg log at all, which is exactly the "X won't start, no logs"
    // symptom. Surface those into dmesg so the failing library/symbol is named.
    let ld_error = has(b"Error loading shared library")
        || has(b"Error relocating")
        || has(b"symbol not found")
        || has(b"No such file")
        || has(b"cannot open shared object")
        || has(b"version `")
        || has(b"undefined symbol");
    if x_marker || ld_error {
        if let Ok(s) = core::str::from_utf8(scan) {
            for line in s.split('\n').filter(|l| !l.is_empty()).take(6) {
                kernel_hal::klog_info!("XLOG: {}", line);
            }
        }
    }
}

/// `struct flock` from `<fcntl.h>` (x86-64/generic 64-bit layout: the i16
/// pair, 4 bytes padding, two i64s, an i32 and tail padding — 32 bytes).
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct Flock {
    /// F_RDLCK / F_WRLCK / F_UNLCK.
    pub l_type: i16,
    /// SEEK_SET / SEEK_CUR / SEEK_END base for `l_start`.
    pub l_whence: i16,
    /// Offset relative to `l_whence`.
    pub l_start: i64,
    /// Byte count; 0 = to EOF and beyond, negative = the range before start.
    pub l_len: i64,
    /// Owner pid, filled in by F_GETLK.
    pub l_pid: i32,
}

/// Round an `F_SETPIPE_SZ` request the way pipe(7) documents: up to the next
/// power of two, never below one page, refused above `fs.pipe-max-size`
/// (1 MiB, the value `/proc/sys/fs/pipe-max-size` advertises) with `EPERM`
/// and refused entirely for zero with `EINVAL`. Pure, so the rounding table
/// is unit-testable.
fn pipe_size_round(arg: usize) -> Result<usize, LxError> {
    const PIPE_MIN_SIZE: usize = 4096;
    const PIPE_MAX_SIZE: usize = 1024 * 1024;
    if arg == 0 {
        return Err(LxError::EINVAL);
    }
    // checked_: a request near usize::MAX has no next power of two, and the
    // unchecked variant would panic the kernel on user-controlled input.
    let size = arg
        .max(PIPE_MIN_SIZE)
        .checked_next_power_of_two()
        .ok_or(LxError::EPERM)?;
    if size > PIPE_MAX_SIZE {
        return Err(LxError::EPERM);
    }
    Ok(size)
}

#[cfg(test)]
mod pipe_size_tests {
    use super::*;

    #[test]
    fn rounds_up_to_powers_of_two_with_page_floor() {
        assert_eq!(pipe_size_round(1), Ok(4096));
        assert_eq!(pipe_size_round(4096), Ok(4096));
        assert_eq!(pipe_size_round(4097), Ok(8192));
        assert_eq!(pipe_size_round(65536), Ok(65536));
        assert_eq!(pipe_size_round(1024 * 1024), Ok(1024 * 1024));
    }

    #[test]
    fn rejects_zero_and_oversize() {
        assert_eq!(pipe_size_round(0), Err(LxError::EINVAL));
        assert_eq!(pipe_size_round(1024 * 1024 + 1), Err(LxError::EPERM));
        assert_eq!(pipe_size_round(usize::MAX), Err(LxError::EPERM));
    }
}
