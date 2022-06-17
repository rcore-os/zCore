//! Linux error codes
use core::fmt;
use rcore_fs::vfs::FsError;
use zircon_object::ZxError;

/// Linux Result defination
pub type LxResult<T = ()> = Result<T, LxError>;
/// SysResult Result defination (same as Linux Result)
pub type SysResult = LxResult<usize>;

/// Linux error codes defination
#[allow(dead_code)]
#[repr(isize)]
#[derive(Debug)]
pub enum LxError {
    /// Undefined
    EUNDEF = 0,
    /// Operation not permitted
    EPERM = 1,
    /// No such file or directory
    ENOENT = 2,
    /// No such process
    ESRCH = 3,
    /// Interrupted system call
    EINTR = 4,
    /// I/O error
    EIO = 5,
    /// No such device or address
    ENXIO = 6,
    /// Arg list too long
    E2BIG = 7,
    /// Exec format error
    ENOEXEC = 8,
    /// Bad file number
    EBADF = 9,
    /// No child processes
    ECHILD = 10,
    /// Try again
    EAGAIN = 11,
    /// Out of memory
    ENOMEM = 12,
    /// Permission denied
    EACCES = 13,
    /// Bad address
    EFAULT = 14,
    /// Block device required
    ENOTBLK = 15,
    /// Device or resource busy
    EBUSY = 16,
    /// File exists
    EEXIST = 17,
    /// Cross-device link
    EXDEV = 18,
    /// No such device
    ENODEV = 19,
    /// Not a directory
    ENOTDIR = 20,
    /// Is a directory
    EISDIR = 21,
    /// Invalid argument
    EINVAL = 22,
    /// File table overflow
    ENFILE = 23,
    /// Too many open files
    EMFILE = 24,
    /// Not a tty device
    ENOTTY = 25,
    /// Text file busy
    ETXTBSY = 26,
    /// File too large
    EFBIG = 27,
    /// No space left on device
    ENOSPC = 28,
    /// Illegal seek
    ESPIPE = 29,
    /// Read-only file system
    EROFS = 30,
    /// Too many links
    EMLINK = 31,
    /// Broken pipe
    EPIPE = 32,
    /// Math argument out of domain
    EDOM = 33,
    /// Math result not representable
    ERANGE = 34,
    /// Resource deadlock would occur
    EDEADLK = 35,
    /// Filename too long
    ENAMETOOLONG = 36,
    /// No record locks available
    ENOLCK = 37,
    /// Function not implemented
    ENOSYS = 38,
    /// Directory not empty
    ENOTEMPTY = 39,
    /// Too many symbolic links encountered
    ELOOP = 40,
    /// Identifier removed
    EIDRM = 43,
    /// Socket operation on non-socket
    ENOTSOCK = 88,
    /// Protocol not available
    ENOPROTOOPT = 92,
    /// Protocol family not supported
    EPFNOSUPPORT = 96,
    /// Address family not supported by protocol
    EAFNOSUPPORT = 97,
    /// No buffer space available
    ENOBUFS = 105,
    /// Transport endpoint is already connected
    EISCONN = 106,
    /// Transport endpoint is not connected
    ENOTCONN = 107,
    /// Connection timeout
    ETIMEDOUT = 110,
    /// Connection refused
    ECONNREFUSED = 111,
}

#[allow(non_snake_case)]
impl fmt::Display for LxError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use self::LxError::*;
        let explain = match self {
            EPERM => "Operation not permitted",
            ENOENT => "No such file or directory",
            ESRCH => "No such process",
            EINTR => "Interrupted system call",
            EIO => "I/O error",
            ENXIO => "No such device or address",
            E2BIG => "Argument list too long",
            ENOEXEC => "Exec format error",
            EBADF => "Bad file number",
            ECHILD => "No child processes",
            EAGAIN => "Try again",
            ENOMEM => "Out of memory",
            EACCES => "Permission denied",
            EFAULT => "Bad address",
            ENOTBLK => "Block device required",
            EBUSY => "Device or resource busy",
            EEXIST => "File exists",
            EXDEV => "Cross-device link",
            ENODEV => "No such device",
            ENOTDIR => "Not a directory",
            EISDIR => "Is a directory",
            EINVAL => "Invalid argument",
            ENFILE => "File table overflow",
            EMFILE => "Too many open files",
            ENOTTY => "Not a typewriter",
            ETXTBSY => "Text file busy",
            EFBIG => "File too large",
            ENOSPC => "No space left on device",
            ESPIPE => "Illegal seek",
            EROFS => "Read-only file system",
            EMLINK => "Too many links",
            EPIPE => "Broken pipe",
            EDOM => "Math argument out of domain of func",
            ERANGE => "Math result not representable",
            EDEADLK => "Resource deadlock would occur",
            ENAMETOOLONG => "File name too long",
            ENOLCK => "No record locks available",
            ENOSYS => "Function not implemented",
            ENOTEMPTY => "Directory not empty",
            ELOOP => "Too many symbolic links encountered",
            EIDRM => "Identifier removed",
            ENOTSOCK => "Socket operation on non-socket",
            ENOPROTOOPT => "Protocol not available",
            EPFNOSUPPORT => "Protocol family not supported",
            EAFNOSUPPORT => "Address family not supported by protocol",
            ENOBUFS => "No buffer space available",
            EISCONN => "Transport endpoint is already connected",
            ENOTCONN => "Transport endpoint is not connected",
            ECONNREFUSED => "Connection refused",
            _ => "Unknown error",
        };
        write!(f, "{}", explain)
    }
}

impl From<ZxError> for LxError {
    fn from(e: ZxError) -> Self {
        match e {
            ZxError::INVALID_ARGS => LxError::EINVAL,
            ZxError::NOT_SUPPORTED => LxError::ENOSYS,
            ZxError::ALREADY_EXISTS => LxError::EEXIST,
            ZxError::SHOULD_WAIT => LxError::EAGAIN,
            ZxError::PEER_CLOSED => LxError::EPIPE,
            ZxError::BAD_HANDLE => LxError::EBADF,
            ZxError::TIMED_OUT => LxError::ETIMEDOUT,
            ZxError::STOP => LxError::ESRCH,
            ZxError::BAD_STATE => LxError::EAGAIN,
            _ => unimplemented!("unknown error type: {:?}", e),
        }
    }
}

impl From<FsError> for LxError {
    fn from(error: FsError) -> Self {
        match error {
            FsError::NotSupported => LxError::ENOSYS,
            FsError::NotFile => LxError::EISDIR,
            FsError::IsDir => LxError::EISDIR,
            FsError::NotDir => LxError::ENOTDIR,
            FsError::EntryNotFound => LxError::ENOENT,
            FsError::EntryExist => LxError::EEXIST,
            FsError::NotSameFs => LxError::EXDEV,
            FsError::InvalidParam => LxError::EINVAL,
            FsError::NoDeviceSpace => LxError::ENOMEM,
            FsError::DirRemoved => LxError::ENOENT,
            FsError::DirNotEmpty => LxError::ENOTEMPTY,
            FsError::WrongFs => LxError::EINVAL,
            FsError::DeviceError => LxError::EIO,
            FsError::IOCTLError => LxError::EINVAL,
            FsError::NoDevice => LxError::EINVAL,
            FsError::Again => LxError::EAGAIN,
            FsError::SymLoop => LxError::ELOOP,
            FsError::Busy => LxError::EBUSY,
            FsError::Interrupted => LxError::EINTR,
        }
    }
}

use kernel_hal::user::Error;

impl From<Error> for LxError {
    fn from(e: Error) -> Self {
        match e {
            Error::InvalidUtf8 => LxError::EINVAL,
            Error::InvalidPointer => LxError::EFAULT,
            Error::BufferTooSmall => LxError::ENOBUFS,
            Error::InvalidLength => LxError::EINVAL,
            Error::InvalidVectorAddress => LxError::EINVAL,
        }
    }
}
