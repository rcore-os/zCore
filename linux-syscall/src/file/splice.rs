//! `splice(2)` / `tee(2)` / `vmsplice(2)` — pipe-centric data movement.
//!
//! Linux implements these zero-copy by handing pipe-buffer page references
//! around. Here the pipe buffer is a plain byte queue, so the calls move the
//! bytes through a bounded kernel buffer instead: the observable semantics
//! (what lands where, offset handling, the error protocol from the man pages)
//! are preserved, the zero-copy part is not. `coreutils`, `pv`, systemd's
//! journal and busybox all fall back gracefully on kernels without splice, but
//! only after a probing call — answering it for real is one less EINVAL path.

use super::*;

const SPLICE_F_MOVE: usize = 1;
const SPLICE_F_NONBLOCK: usize = 2;
const SPLICE_F_MORE: usize = 4;
const SPLICE_F_GIFT: usize = 8;
const SPLICE_FLAGS_ALL: usize = SPLICE_F_MOVE | SPLICE_F_NONBLOCK | SPLICE_F_MORE | SPLICE_F_GIFT;

/// The inode behind a file-like when it is a pipe. Pipes here are always a
/// `File` wrapping a `Pipe` inode (see `sys_pipe2`); `dyn INode` only offers
/// `downcast_ref`, so the caller keeps the returned `Arc` alive and borrows
/// the `Pipe` out of it at the use site.
fn pipe_inode(f: &Arc<dyn FileLike>) -> Option<Arc<dyn rcore_fs::vfs::INode>> {
    let file = f.downcast_ref::<File>()?;
    let inode = file.inode();
    inode.downcast_ref::<Pipe>().is_some().then_some(inode)
}

impl Syscall<'_> {
    /// splice data to/from a pipe
    /// (see [linux man splice(2)](https://www.man7.org/linux/man-pages/man2/splice.2.html)).
    ///
    /// Moves up to `len` bytes from `fd_in` to `fd_out`, where at least one of
    /// the two must be a pipe (`EINVAL` otherwise) and an offset may only be
    /// given for the non-pipe side (`ESPIPE` otherwise), exactly the man-page
    /// contract. `SPLICE_F_NONBLOCK` makes the pipe side answer `EAGAIN`
    /// instead of waiting; MOVE/MORE/GIFT are hints and accepted as such.
    pub async fn sys_splice(
        &self,
        fd_in: FileDesc,
        off_in: UserInOutPtr<i64>,
        fd_out: FileDesc,
        off_out: UserInOutPtr<i64>,
        len: usize,
        flags: usize,
    ) -> SysResult {
        info!(
            "splice: fd_in={:?}, off_in={:?}, fd_out={:?}, off_out={:?}, len={}, flags={:#x}",
            fd_in, off_in, fd_out, off_out, len, flags
        );
        if flags & !SPLICE_FLAGS_ALL != 0 {
            return Err(LxError::EINVAL);
        }
        let proc = self.linux_process();
        let file_in = proc.get_file_like(fd_in)?;
        let file_out = proc.get_file_like(fd_out)?;
        let in_is_pipe = pipe_inode(&file_in).is_some();
        let out_is_pipe = pipe_inode(&file_out).is_some();
        if !in_is_pipe && !out_is_pipe {
            return Err(LxError::EINVAL);
        }
        if (in_is_pipe && !off_in.is_null()) || (out_is_pipe && !off_out.is_null()) {
            return Err(LxError::ESPIPE);
        }
        if len == 0 {
            return Ok(0);
        }
        let len = len.min(super::SYSCALL_IO_MAX);
        let mut buf = vec![0u8; len];

        // SPLICE_F_NONBLOCK: the pipe side must not wait. `File::read` already
        // branches on the fd's NON_BLOCK flag, so borrow the recvmsg trick and
        // set it for the duration of the call.
        let force_nonblock = flags & SPLICE_F_NONBLOCK != 0
            && in_is_pipe
            && !file_in.flags().contains(OpenFlags::NON_BLOCK);
        let old_flags = file_in.flags();
        if force_nonblock {
            file_in.set_flags(old_flags | OpenFlags::NON_BLOCK)?;
        }
        let read_result = if !off_in.is_null() {
            let off = off_in.read()?;
            if off < 0 {
                return Err(LxError::EINVAL);
            }
            let res = file_in.read_at(off as u64, &mut buf).await;
            if let Ok(n) = res {
                if n > 0 {
                    // The fd's own file offset stays untouched — only *off_in
                    // advances, as documented.
                    let mut off_in = off_in;
                    off_in.write(off + n as i64)?;
                }
            }
            res
        } else {
            file_in.read(&mut buf).await
        };
        if force_nonblock {
            let _ = file_in.set_flags(old_flags);
        }
        let n = read_result?;
        if n == 0 {
            return Ok(0);
        }

        if !off_out.is_null() {
            let off = off_out.read()?;
            if off < 0 {
                return Err(LxError::EINVAL);
            }
            let written = file_out.write_at(off as u64, &buf[..n])?;
            if written > 0 {
                let mut off_out = off_out;
                off_out.write(off + written as i64)?;
            }
            Ok(written)
        } else {
            file_out.write(&buf[..n])
        }
    }

    /// duplicate pipe content
    /// (see [linux man tee(2)](https://www.man7.org/linux/man-pages/man2/tee.2.html)).
    ///
    /// Copies up to `len` bytes from the read end `fd_in` to the write end
    /// `fd_out` **without consuming** the input — the defining property of
    /// `tee`, served by `Pipe::peek_data`. Both fds must be pipes (`EINVAL`).
    /// An empty input pipe blocks until data or writer hang-up unless
    /// `SPLICE_F_NONBLOCK` asks for `EAGAIN`.
    pub async fn sys_tee(
        &self,
        fd_in: FileDesc,
        fd_out: FileDesc,
        len: usize,
        flags: usize,
    ) -> SysResult {
        info!(
            "tee: fd_in={:?}, fd_out={:?}, len={}, flags={:#x}",
            fd_in, fd_out, len, flags
        );
        if flags & !SPLICE_FLAGS_ALL != 0 {
            return Err(LxError::EINVAL);
        }
        let proc = self.linux_process();
        let file_in = proc.get_file_like(fd_in)?;
        let file_out = proc.get_file_like(fd_out)?;
        let inode_in = pipe_inode(&file_in).ok_or(LxError::EINVAL)?;
        let inode_out = pipe_inode(&file_out).ok_or(LxError::EINVAL)?;
        let pipe_in = inode_in.downcast_ref::<Pipe>().ok_or(LxError::EINVAL)?;
        let pipe_out = inode_out.downcast_ref::<Pipe>().ok_or(LxError::EINVAL)?;
        if !pipe_in.is_read_end() || pipe_out.is_read_end() {
            return Err(LxError::EBADF);
        }
        if len == 0 {
            return Ok(0);
        }
        let len = len.min(super::SYSCALL_IO_MAX);
        loop {
            let (data, writers_alive) = pipe_in.peek_data(len).ok_or(LxError::EBADF)?;
            if !data.is_empty() {
                return file_out.write(&data);
            }
            if !writers_alive {
                return Ok(0);
            }
            if flags & SPLICE_F_NONBLOCK != 0 {
                return Err(LxError::EAGAIN);
            }
            // Wait for data or writer hang-up, then re-check.
            file_in.async_poll(PollEvents::IN).await?;
        }
    }

    /// splice user pages to/from a pipe
    /// (see [linux man vmsplice(2)](https://www.man7.org/linux/man-pages/man2/vmsplice.2.html)).
    ///
    /// On the write end the iovecs are gathered into the pipe; on the read end
    /// the pipe is scattered into the iovecs. Without page-reference pipe
    /// buffers this is exactly `writev`/`readv` on the pipe fd — including for
    /// `SPLICE_F_GIFT`, which is only ever an optimisation hint.
    pub async fn sys_vmsplice(
        &self,
        fd: FileDesc,
        iov_ptr: usize,
        iov_count: usize,
        flags: usize,
    ) -> SysResult {
        info!(
            "vmsplice: fd={:?}, iov={:#x}, count={}, flags={:#x}",
            fd, iov_ptr, iov_count, flags
        );
        if flags & !SPLICE_FLAGS_ALL != 0 {
            return Err(LxError::EINVAL);
        }
        let proc = self.linux_process();
        let file = proc.get_file_like(fd)?;
        let inode = pipe_inode(&file).ok_or(LxError::EBADF)?;
        let is_read_end = inode
            .downcast_ref::<Pipe>()
            .map(Pipe::is_read_end)
            .unwrap_or(false);
        if is_read_end {
            self.sys_readv(fd, iov_ptr.into(), iov_count).await
        } else {
            self.sys_writev(fd, iov_ptr.into(), iov_count)
        }
    }
}
