static mut HEAP: Heap = Heap([0; 1048576 * 16]);
static mut HEAP_TOP: usize = 0;
const PAGE_SIZE: usize = 4096;

#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

#[repr(align(4096))]
struct Heap([u8; 1048576 * 16]);

pub fn init() {
    unsafe {
        HEAP_TOP = heap_start();
        assert!(HEAP_TOP % PAGE_SIZE == 0);
    }
    println!("sel4/allocator: Initialized.");
}

pub fn heap_start() -> usize {
    unsafe {
        &mut HEAP.0 as *mut _ as usize
    }
}

pub fn heap_end() -> usize {
    heap_start() + unsafe { HEAP.0.len() }
}

pub fn heap_usage() -> usize {
    unsafe { HEAP_TOP - heap_start() }
}

#[alloc_error_handler]
fn on_alloc_error(_: core::alloc::Layout) -> ! {
    panic!("Allocation failed");
}

#[no_mangle]
extern "C" fn __dlmalloc_alloc(size: usize) -> usize {
    println!("dlmalloc_alloc {}. HEAP_TOP = {}", size, unsafe { HEAP_TOP });
    let old_top = unsafe { HEAP_TOP };
    match old_top.checked_add(size) {
        Some(x) if x <= heap_end() => {
            unsafe {
                // dlmalloc zeros allocated memory.
                HEAP_TOP = x;
            }
            old_top
        }
        _ => usize::MAX,
    }
}

#[no_mangle]
extern "C" fn __dlmalloc_acquire_global_lock() {
}

#[no_mangle]
extern "C" fn __dlmalloc_release_global_lock() {
}

#[no_mangle]
static __DLMALLOC_PAGE_SIZE: usize = PAGE_SIZE;
