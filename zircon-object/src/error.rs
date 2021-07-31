/// The type returned by kernel objects methods.
pub type ZxResult<T = ()> = Result<T, ZxError>;

/// Zircon statuses are signed 32 bit integers. The space of values is
/// divided as follows:
/// - The zero value is for the OK status.
/// - Negative values are defined by the system, in this file.
/// - Positive values are reserved for protocol-specific error values,
///   and will never be defined by the system.
#[allow(non_camel_case_types)]
#[allow(clippy::upper_case_acronyms)]
#[repr(i32)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ZxError {
    /// Success.
    OK = 0,

    // ======= Internal failures =======
    /// The system encountered an otherwise unspecified error
    /// while performing the operation.
    INTERNAL = -1,

    /// The operation is not implemented, supported,
    /// or enabled.
    NOT_SUPPORTED = -2,

    /// The system was not able to allocate some resource
    /// needed for the operation.
    NO_RESOURCES = -3,

    /// The system was not able to allocate memory needed
    /// for the operation.
    NO_MEMORY = -4,

    // -5 used to be ZX_ERR_CALL_FAILED.
    /// The system call was interrupted, but should be
    /// retried.  This should not be seen outside of the VDSO.
    INTERNAL_INTR_RETRY = -6,

    // ======= Parameter errors =======
    /// an argument is invalid, ex. null pointer
    INVALID_ARGS = -10,

    /// A specified handle value does not refer to a handle.
    BAD_HANDLE = -11,

    /// The subject of the operation is the wrong type to
    /// perform the operation.
    /// Example: Attempting a message_read on a thread handle.
    WRONG_TYPE = -12,

    /// The specified syscall number is invalid.
    BAD_SYSCALL = -13,

    /// An argument is outside the valid range for this
    /// operation.
    OUT_OF_RANGE = -14,

    /// A caller provided buffer is too small for
    /// this operation.
    BUFFER_TOO_SMALL = -15,

    // ======= Precondition or state errors =======
    /// operation failed because the current state of the
    /// object does not allow it, or a precondition of the operation is
    /// not satisfied
    BAD_STATE = -20,

    /// The time limit for the operation elapsed before
    /// the operation completed.
    TIMED_OUT = -21,

    /// The operation cannot be performed currently but
    /// potentially could succeed if the caller waits for a prerequisite
    /// to be satisfied, for example waiting for a handle to be readable
    /// or writable.
    /// Example: Attempting to read from a channel that has no
    /// messages waiting but has an open remote will return ZX_ERR_SHOULD_WAIT.
    /// Attempting to read from a channel that has no messages waiting
    /// and has a closed remote end will return ZX_ERR_PEER_CLOSED.
    SHOULD_WAIT = -22,

    /// The in-progress operation (e.g. a wait) has been
    /// canceled.
    CANCELED = -23,

    /// The operation failed because the remote end of the
    /// subject of the operation was closed.
    PEER_CLOSED = -24,

    /// The requested entity is not found.
    NOT_FOUND = -25,

    /// An object with the specified identifier
    /// already exists.
    /// Example: Attempting to create a file when a file already exists
    /// with that name.
    ALREADY_EXISTS = -26,

    /// The operation failed because the named entity
    /// is already owned or controlled by another entity. The operation
    /// could succeed later if the current owner releases the entity.
    ALREADY_BOUND = -27,

    /// The subject of the operation is currently unable
    /// to perform the operation.
    /// Note: This is used when there's no direct way for the caller to
    /// observe when the subject will be able to perform the operation
    /// and should thus retry.
    UNAVAILABLE = -28,

    // ======= Permission check errors =======
    /// The caller did not have permission to perform
    /// the specified operation.
    ACCESS_DENIED = -30,

    // ======= Input-output errors =======
    /// Otherwise unspecified error occurred during I/O.
    IO = -40,

    /// The entity the I/O operation is being performed on
    /// rejected the operation.
    /// Example: an I2C device NAK'ing a transaction or a disk controller
    /// rejecting an invalid command, or a stalled USB endpoint.
    IO_REFUSED = -41,

    /// The data in the operation failed an integrity
    /// check and is possibly corrupted.
    /// Example: CRC or Parity error.
    IO_DATA_INTEGRITY = -42,

    /// The data in the operation is currently unavailable
    /// and may be permanently lost.
    /// Example: A disk block is irrecoverably damaged.
    IO_DATA_LOSS = -43,

    /// The device is no longer available (has been
    /// unplugged from the system, powered down, or the driver has been
    /// unloaded,
    IO_NOT_PRESENT = -44,

    /// More data was received from the device than expected.
    /// Example: a USB "babble" error due to a device sending more data than
    /// the host queued to receive.
    IO_OVERRUN = -45,

    /// An operation did not complete within the required timeframe.
    /// Example: A USB isochronous transfer that failed to complete due to an overrun or underrun.
    IO_MISSED_DEADLINE = -46,

    /// The data in the operation is invalid parameter or is out of range.
    /// Example: A USB transfer that failed to complete with TRB Error
    IO_INVALID = -47,

    // ======== Filesystem Errors ========
    /// Path name is too long.
    BAD_PATH = -50,

    /// Object is not a directory or does not support
    /// directory operations.
    /// Example: Attempted to open a file as a directory or
    /// attempted to do directory operations on a file.
    NOT_DIR = -51,

    /// Object is not a regular file.
    NOT_FILE = -52,

    /// This operation would cause a file to exceed a
    /// filesystem-specific size limit
    FILE_BIG = -53,

    /// Filesystem or device space is exhausted.
    NO_SPACE = -54,

    /// Directory is not empty.
    NOT_EMPTY = -55,

    // ======== Flow Control ========
    // These are not errors, as such, and will never be returned
    // by a syscall or public API.  They exist to allow callbacks
    // to request changes in operation.
    /// Do not call again.
    /// Example: A notification callback will be called on every
    /// event until it returns something other than ZX_OK.
    /// This status allows differentiation between "stop due to
    /// an error" and "stop because the work is done."
    STOP = -60,

    /// Advance to the next item.
    /// Example: A notification callback will use this response
    /// to indicate it did not "consume" an item passed to it,
    /// but by choice, not due to an error condition.
    NEXT = -61,

    /// Ownership of the item has moved to an asynchronous worker.
    ///
    /// Unlike ZX_ERR_STOP, which implies that iteration on an object
    /// should stop, and ZX_ERR_NEXT, which implies that iteration
    /// should continue to the next item, ZX_ERR_ASYNC implies
    /// that an asynchronous worker is responsible for continuing iteration.
    ///
    /// Example: A notification callback will be called on every
    /// event, but one event needs to handle some work asynchronously
    /// before it can continue. ZX_ERR_ASYNC implies the worker is
    /// responsible for resuming iteration once its work has completed.
    ASYNC = -62,

    // ======== Network-related errors ========
    /// Specified protocol is not
    /// supported.
    PROTOCOL_NOT_SUPPORTED = -70,

    /// Host is unreachable.
    ADDRESS_UNREACHABLE = -71,

    /// Address is being used by someone else.
    ADDRESS_IN_USE = -72,

    /// Socket is not connected.
    NOT_CONNECTED = -73,

    /// Remote peer rejected the connection.
    CONNECTION_REFUSED = -74,

    /// Connection was reset.
    CONNECTION_RESET = -75,

    /// Connection was aborted.
    CONNECTION_ABORTED = -76,
}

use kernel_hal::user::Error;

impl From<Error> for ZxError {
    fn from(e: Error) -> Self {
        match e {
            Error::InvalidUtf8 => ZxError::INVALID_ARGS,
            Error::InvalidPointer => ZxError::INVALID_ARGS,
            Error::BufferTooSmall => ZxError::BUFFER_TOO_SMALL,
            Error::InvalidLength => ZxError::INVALID_ARGS,
            Error::InvalidVectorAddress => ZxError::NOT_FOUND,
        }
    }
}
