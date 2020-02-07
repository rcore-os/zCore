use core::fmt;
use rcore_fs::vfs::FsError;
use zircon_object::ZxError;

pub type LxResult<T> = Result<T, LxError>;
pub type SysResult = LxResult<usize>;

#[allow(dead_code)]
#[repr(isize)]
#[derive(Debug)]
pub enum LxError {
    EUNDEF = 0,
    EPERM = 1,
    ENOENT = 2,
    ESRCH = 3,
    EINTR = 4,
    EIO = 5,
    ENXIO = 6,
    E2BIG = 7,
    ENOEXEC = 8,
    EBADF = 9,
    ECHILD = 10,
    EAGAIN = 11,
    ENOMEM = 12,
    EACCES = 13,
    EFAULT = 14,
    ENOTBLK = 15,
    EBUSY = 16,
    EEXIST = 17,
    EXDEV = 18,
    ENODEV = 19,
    ENOTDIR = 20,
    EISDIR = 21,
    EINVAL = 22,
    ENFILE = 23,
    EMFILE = 24,
    ENOTTY = 25,
    ETXTBSY = 26,
    EFBIG = 27,
    ENOSPC = 28,
    ESPIPE = 29,
    EROFS = 30,
    EMLINK = 31,
    EPIPE = 32,
    EDOM = 33,
    ERANGE = 34,
    EDEADLK = 35,
    ENAMETOOLONG = 36,
    ENOLCK = 37,
    ENOSYS = 38,
    ENOTEMPTY = 39,
    ELOOP = 40,
    ENOTSOCK = 80,
    ENOPROTOOPT = 92,
    EPFNOSUPPORT = 96,
    EAFNOSUPPORT = 97,
    ENOBUFS = 105,
    EISCONN = 106,
    ENOTCONN = 107,
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
        }
    }
}

use kernel_hal::user::Error;

impl From<Error> for LxError {
    fn from(e: Error) -> Self {
        match e {
            Error::InvalidUtf8 => LxError::EINVAL,
            Error::InvalidPointer => LxError::EFAULT,
        }
    }
}
