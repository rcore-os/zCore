use kernel_hal::MMUFlags;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
};
use zircon_object::{
    dev::{Resource, ResourceFlags, ResourceKind},
    ipc::Channel,
    object::{Handle, Rights},
    signal::Event,
    task::{CurrentThread, Job, Process, Thread, ThreadFn},
    vm::{VmObject, PAGE_SIZE},
    ZxError,
};
use zircon_syscall::Syscall;

// Thread::start invokes the callback synchronously, providing a CurrentThread
// with a real process address space for syscall pointer validation.
type ThreadFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
fn finish_thread(ct: CurrentThread) -> ThreadFuture {
    Box::pin(async move { drop(ct) })
}
fn with_test_thread(f: ThreadFn) {
    let proc = Process::create(&Job::root(), "syscall-regression").unwrap();
    Thread::create(&proc, "syscall-regression")
        .unwrap()
        .start(f)
        .unwrap();
}
fn map_user_memory(ct: &CurrentThread) -> usize {
    let vmo = VmObject::new_paged(20);
    ct.proc()
        .vmar()
        .map(
            None,
            vmo,
            0,
            20 * PAGE_SIZE,
            MMUFlags::USER | MMUFlags::READ | MMUFlags::WRITE,
        )
        .unwrap()
}
#[repr(C)]
struct ChannelCallArgs {
    wr_bytes: usize,
    wr_handles: usize,
    rd_bytes: usize,
    rd_handles: usize,
    wr_num_bytes: u32,
    wr_num_handles: u32,
    rd_num_bytes: u32,
    rd_num_handles: u32,
}
fn poll_call(sc: &mut Syscall<'_>, h: u32, base: usize) -> Poll<isize> {
    let mut f = Box::pin(sc.syscall(
        4,
        [
            h as usize,
            0,
            i64::MAX as usize,
            base,
            base + 128,
            base + 132,
            0,
            0,
        ],
    ));
    f.as_mut().poll(&mut Context::from_waker(Waker::noop()))
}
#[test]
fn call_etc_rejects_oversized_message() {
    with_test_thread(|ct| {
        let base = map_user_memory(&ct);
        let (a, b) = Channel::create();
        let h = ct
            .proc()
            .add_handle(Handle::new(a, Rights::DEFAULT_CHANNEL));
        unsafe {
            (base as *mut ChannelCallArgs).write(ChannelCallArgs {
                wr_bytes: base + 4096,
                wr_handles: 0,
                rd_bytes: base + 256,
                rd_handles: 0,
                wr_num_bytes: 65537,
                wr_num_handles: 0,
                rd_num_bytes: 4,
                rd_num_handles: 0,
            });
        }
        let mut sc = Syscall {
            thread: &ct,
            thread_fn: finish_thread,
        };
        let result = poll_call(&mut sc, h, base);
        let delivered = b.read().ok().map(|m| m.data.len());
        assert_eq!(
            result,
            Poll::Ready(ZxError::OUT_OF_RANGE as isize),
            "delivered={:?}",
            delivered
        );
        Box::pin(async move { drop(ct) })
    });
}
#[test]
fn call_etc_closes_moved_handle_on_wrong_type() {
    with_test_thread(|ct| {
        let base = map_user_memory(&ct);
        let (a, _b) = Channel::create();
        let h = ct
            .proc()
            .add_handle(Handle::new(a, Rights::DEFAULT_CHANNEL));
        let event = ct
            .proc()
            .add_handle(Handle::new(Event::new(), Rights::DEFAULT_EVENT));
        unsafe {
            (base as *mut ChannelCallArgs).write(ChannelCallArgs {
                wr_bytes: base + 4096,
                wr_handles: base + 256,
                rd_bytes: base + 512,
                rd_handles: 0,
                wr_num_bytes: 4,
                wr_num_handles: 1,
                rd_num_bytes: 4,
                rd_num_handles: 0,
            });
            ((base + 256) as *mut [u32; 5]).write([
                0,
                event,
                u32::MAX,
                Rights::SAME_RIGHTS.bits(),
                0,
            ]);
        }
        let mut sc = Syscall {
            thread: &ct,
            thread_fn: finish_thread,
        };
        assert_eq!(
            poll_call(&mut sc, h, base),
            Poll::Ready(ZxError::WRONG_TYPE as isize)
        );
        assert!(
            ct.proc().get_object::<Event>(event).is_err(),
            "MOVE handle still valid after WRONG_TYPE"
        );
        Box::pin(async move { drop(ct) })
    });
}
#[test]
fn unrelated_system_resource_cannot_authorize_vmex() {
    with_test_thread(|ct| {
        let vmo = ct
            .proc()
            .add_handle(Handle::new(VmObject::new_paged(1), Rights::DEFAULT_VMO));
        let resource = ct.proc().add_handle(Handle::new(
            Resource::create(
                "debuglog",
                ResourceKind::SYSTEM,
                12,
                1,
                ResourceFlags::empty(),
            ),
            Rights::DEFAULT_RESOURCE,
        ));
        let mut out = 0u32;
        let sc = Syscall {
            thread: &ct,
            thread_fn: finish_thread,
        };
        let result =
            sc.sys_vmo_replace_as_executable(vmo, resource, (&mut out as *mut u32 as usize).into());
        assert!(
            result.is_err(),
            "debuglog resource granted executable handle {:?}",
            out
        );
        Box::pin(async move { drop(ct) })
    });
}

#[test]
fn call_limits_apply_to_both_abis_and_iovecs() {
    with_test_thread(|ct| {
        let base = map_user_memory(&ct);
        for syscall in [4, 6] {
            for (options, bytes, handles) in [(0, 65537, 0), (0, 4, 65), (2, 1, 65)] {
                let (a, b) = Channel::create();
                let channel = ct
                    .proc()
                    .add_handle(Handle::new(a, Rights::DEFAULT_CHANNEL));
                let mut moved = Vec::new();
                for i in 0..handles as usize {
                    let h = ct
                        .proc()
                        .add_handle(Handle::new(Event::new(), Rights::DEFAULT_EVENT));
                    moved.push(h);
                    unsafe {
                        if syscall == 4 {
                            ((base + 1024 + i * 20) as *mut [u32; 5]).write([
                                0,
                                h,
                                0,
                                Rights::SAME_RIGHTS.bits(),
                                0,
                            ]);
                        } else {
                            ((base + 1024 + i * 4) as *mut u32).write(h);
                        }
                    }
                }
                unsafe {
                    (base as *mut ChannelCallArgs).write(ChannelCallArgs {
                        wr_bytes: base + 4096,
                        wr_handles: base + 1024,
                        rd_bytes: base + 512,
                        rd_handles: 0,
                        wr_num_bytes: bytes,
                        wr_num_handles: handles,
                        rd_num_bytes: 4,
                        rd_num_handles: 0,
                    });
                    if options == 2 {
                        ((base + 4096) as *mut [usize; 2]).write([base + 8192, 4]);
                    }
                }
                let mut sc = Syscall {
                    thread: &ct,
                    thread_fn: finish_thread,
                };
                let mut f = Box::pin(sc.syscall(
                    syscall,
                    [
                        channel as usize,
                        options,
                        i64::MAX as usize,
                        base,
                        base + 128,
                        base + 132,
                        0,
                        0,
                    ],
                ));
                assert_eq!(
                    f.as_mut().poll(&mut Context::from_waker(Waker::noop())),
                    Poll::Ready(ZxError::OUT_OF_RANGE as isize)
                );
                assert!(b.read().is_err());
                for h in moved {
                    assert!(ct.proc().get_object::<Event>(h).is_err());
                }
            }
        }
        Box::pin(async move { drop(ct) })
    });
}

#[test]
fn failure_consumes_all_moves_but_preserves_duplicates() {
    with_test_thread(|ct| {
        let base = map_user_memory(&ct);
        let (a, b) = Channel::create();
        let channel = ct
            .proc()
            .add_handle(Handle::new(a, Rights::DEFAULT_CHANNEL));
        let moved = ct
            .proc()
            .add_handle(Handle::new(Event::new(), Rights::DEFAULT_EVENT));
        let duplicate = ct
            .proc()
            .add_handle(Handle::new(Event::new(), Rights::DEFAULT_EVENT));
        let later_move = ct
            .proc()
            .add_handle(Handle::new(Event::new(), Rights::DEFAULT_EVENT));
        unsafe {
            (base as *mut ChannelCallArgs).write(ChannelCallArgs {
                wr_bytes: base + 4096,
                wr_handles: base + 256,
                rd_bytes: base + 512,
                rd_handles: 0,
                wr_num_bytes: 4,
                wr_num_handles: 3,
                rd_num_bytes: 4,
                rd_num_handles: 0,
            });
            ((base + 256) as *mut [[u32; 5]; 3]).write([
                [0, moved, u32::MAX, Rights::SAME_RIGHTS.bits(), 0],
                [1, duplicate, u32::MAX, Rights::SAME_RIGHTS.bits(), 0],
                [0, later_move, 0, Rights::SAME_RIGHTS.bits(), 0],
            ]);
        }
        let mut sc = Syscall {
            thread: &ct,
            thread_fn: finish_thread,
        };
        assert_eq!(
            poll_call(&mut sc, channel, base),
            Poll::Ready(ZxError::WRONG_TYPE as isize)
        );
        assert!(ct.proc().get_object::<Event>(moved).is_err());
        assert!(ct.proc().get_object::<Event>(later_move).is_err());
        assert!(ct.proc().get_object::<Event>(duplicate).is_ok());
        assert!(b.read().is_err());
        unsafe {
            let result = ((base + 256) as *const [[u32; 5]; 3]).read();
            assert_eq!(result[0][4] as i32, ZxError::WRONG_TYPE as i32);
            assert_eq!(result[1][4] as i32, ZxError::WRONG_TYPE as i32);
            assert_eq!(result[2][4], 0);
        }
        Box::pin(async move { drop(ct) })
    });
}

#[test]
fn info_versions_preserve_short_buffer_counts_and_boundaries() {
    with_test_thread(|ct| {
        let base = map_user_memory(&ct);
        let vmo = ct
            .proc()
            .add_handle(Handle::new(VmObject::new_paged(1), Rights::DEFAULT_VMO));
        let sc = Syscall {
            thread: &ct,
            thread_fn: finish_thread,
        };
        for (topic, size) in [
            (23, 104),
            (0x1000_0017, 120),
            (0x2000_0017, 128),
            (
                0x3000_0017,
                core::mem::size_of::<zircon_object::vm::VmoInfo>(),
            ),
        ] {
            let actual = base + 512;
            let avail = base + 520;
            unsafe {
                core::ptr::write_bytes(base as *mut u8, 0xa5, 1024);
            }
            assert_eq!(
                sc.sys_object_get_info(vmo, topic, base, size - 1, actual.into(), avail.into()),
                Err(ZxError::BUFFER_TOO_SMALL)
            );
            unsafe {
                assert_eq!((actual as *const usize).read(), 0);
                assert_eq!((avail as *const usize).read(), 1);
                assert_eq!((base as *const u8).read(), 0xa5);
            }
            sc.sys_object_get_info(vmo, topic, base, size, actual.into(), avail.into())
                .unwrap();
            unsafe {
                assert_eq!((actual as *const usize).read(), 1);
                assert_eq!((avail as *const usize).read(), 1);
                assert_eq!(((base + size) as *const u8).read(), 0xa5);
            }
            sc.sys_object_get_info(vmo, topic, base, size, 0.into(), 0.into())
                .unwrap();
        }
        Box::pin(async move { drop(ct) })
    });
}
