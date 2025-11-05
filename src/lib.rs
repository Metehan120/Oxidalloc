use libc::{MADV_DONTNEED, MADV_HUGEPAGE, madvise, munmap, pthread_key_t, size_t};
use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

// Global map list for shared memory allocation
static GLOBAL_MAP_LIST: [AtomicPtr<Header>; 20] = [
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
    AtomicPtr::new(std::ptr::null_mut()),
];

static PTHREAD_KEY: OnceLock<pthread_key_t> = OnceLock::new();
static TOTAL_USED: AtomicUsize = AtomicUsize::new(0);
static TOTAL_ALLOCATED: AtomicUsize = AtomicUsize::new(0);

struct ThreadLocalCache {
    map_list: [AtomicPtr<Header>; 20],
}

fn get_thread_cache() -> &'static ThreadLocalCache {
    unsafe {
        let key = PTHREAD_KEY.get_or_init(|| {
            let mut key = 0;
            libc::pthread_key_create(&mut key, Some(cleanup_thread_cache));
            key
        });

        let cache_ptr = libc::pthread_getspecific(*key) as *mut ThreadLocalCache;

        if cache_ptr.is_null() {
            let cache_ptr = libc::mmap(
                std::ptr::null_mut(),
                std::mem::size_of::<ThreadLocalCache>(),
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            ) as *mut ThreadLocalCache;

            std::ptr::write(
                cache_ptr,
                ThreadLocalCache {
                    map_list: [const { AtomicPtr::new(std::ptr::null_mut()) }; 20],
                },
            );

            libc::pthread_setspecific(*key, cache_ptr as *mut c_void);
            &*cache_ptr
        } else {
            &*cache_ptr
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn push_to_global(class: usize, head: *mut Header, tail: *mut Header) {
    loop {
        let current_head = GLOBAL_MAP_LIST[class].load(Ordering::Acquire);
        (*tail).next = current_head;

        if GLOBAL_MAP_LIST[class]
            .compare_exchange(current_head, head, Ordering::Release, Ordering::Acquire)
            .is_ok()
        {
            return;
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn pop_batch_from_global(class: usize, batch_size: usize) -> *mut Header {
    for _ in 0..2 {
        let current_head = GLOBAL_MAP_LIST[class].load(Ordering::Acquire);

        if current_head.is_null() {
            return null_mut();
        }

        // Walk to find tail of batch
        let mut tail = current_head;
        let mut count = 1;
        while count < batch_size && !(*tail).next.is_null() {
            tail = (*tail).next;
            count += 1;
        }

        let new_head = (*tail).next;

        if GLOBAL_MAP_LIST[class]
            .compare_exchange(current_head, new_head, Ordering::Release, Ordering::Acquire)
            .is_ok()
        {
            (*tail).next = null_mut(); // Detach batch
            return current_head;
        }
    }

    null_mut()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cleanup_thread_cache(cache_ptr: *mut c_void) {
    let cache = cache_ptr as *mut ThreadLocalCache;

    // Move all blocks to global
    for class in 0..20 {
        let head = (*cache).map_list[class].swap(null_mut(), Ordering::AcqRel);

        if !head.is_null() {
            let mut tail = head;
            while !(*tail).next.is_null() {
                tail = (*tail).next;
            }
            push_to_global(class, head, tail);
        }
    }

    // Free the cache itself
    if libc::munmap(cache_ptr, std::mem::size_of::<ThreadLocalCache>()) != 0 {
        madvise(
            cache_ptr,
            std::mem::size_of::<ThreadLocalCache>(),
            MADV_DONTNEED,
        );
    };
}

// ------------------------------------------------------

const HEADER_SIZE: usize = size_of::<Header>();

const MAGIC: u64 = 0x01B01698BF0BEEF;

// Size classes for memory allocation
pub const SIZE_CLASSES: [usize; 20] = [
    512, 4096, 6144, 8192, 10240, 12288, 16384, 32768, 65536, 262144, 1048576, 2097152, 4194304,
    8388608, 12582912, 16777216, 33554432, 67108864, 100663296, 134217728,
];

// Iterations of each size class, each iteration is a try to allocate a chunk of memory
pub const ITERATIONS: [usize; 20] = [
    1024, 1024, 512, 256, 256, 128, 128, 128, 64, 32, 16, 8, 6, 6, 4, 4, 4, 2, 2, 2,
];

static TRIM_COUNTER: AtomicUsize = AtomicUsize::new(0);

const OUR_VA_START: usize = 0x600000000000;
const OUR_VA_END: usize = 0x620000000000;
// TODO: Add optional ASLR-style randomization for VA_OFFSET
// Current: deterministic start at OUR_VA_START
// Future: randomize within range on bootstrap
static VA_OFFSET: AtomicUsize = AtomicUsize::new(OUR_VA_START);

#[repr(C, align(16))]
pub struct Header {
    magic: u64,
    size: u64,
    next: *mut Header,
}

pub fn match_size_class(size: usize) -> Option<usize> {
    match size {
        0..=512 => Some(0),
        513..=4096 => Some(1),
        4097..=6144 => Some(2),
        6145..=8192 => Some(3),
        8193..=10240 => Some(4),
        10241..=12288 => Some(5),
        12289..=16384 => Some(6),
        16385..=32768 => Some(7),
        32769..=65536 => Some(8),
        65537..=262144 => Some(9),
        262145..=1048576 => Some(10),
        1048577..=2097152 => Some(11),
        2097153..=4194304 => Some(12),
        4194305..=8388608 => Some(13),
        8388609..=12582912 => Some(14),
        12582913..=16777216 => Some(15),
        16777217..=33554432 => Some(16),
        33554433..=67108864 => Some(17),
        67108865..=100663296 => Some(18),
        100663297..=134217728 => Some(19),
        _ => None,
    }
}

pub fn big_alloc(size: usize) -> *mut c_void {
    unsafe {
        let total_size = size + HEADER_SIZE;

        let hint = VA_OFFSET.fetch_add(size, Ordering::Relaxed);

        // Offset 0, PROT_READ | PROT_WRITE: Can Write, Can Read
        let chunk = libc::mmap(
            hint as *mut c_void,
            total_size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        );

        if chunk == libc::MAP_FAILED {
            return std::ptr::null_mut();
        }

        // Huge Page optimization:
        // Going to slow down the allocation process but increase performance in usage
        madvise(chunk, total_size, MADV_HUGEPAGE);

        // Header: Initialize the header fields
        let header = chunk as *mut Header;
        (*header).next = std::ptr::null_mut();
        (*header).size = size as u64;
        (*header).magic = MAGIC;

        // Header + Header Size -> Output
        (header as *mut u8).add(HEADER_SIZE) as *mut c_void
    }
}

pub fn bulk_allocate(class: usize) -> bool {
    unsafe {
        let block_size = SIZE_CLASSES[class] + HEADER_SIZE;
        let total_mmap_size = block_size * ITERATIONS[class];

        let hint = VA_OFFSET.fetch_add(total_mmap_size, Ordering::Relaxed);

        // Offset 0, PROT_READ | PROT_WRITE: Can Write, Can Read
        let chunk = libc::mmap(
            hint as *mut c_void,
            total_mmap_size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        );

        if chunk == libc::MAP_FAILED {
            return false;
        }

        // Add allocated memory to the total allocated counter
        TOTAL_ALLOCATED.fetch_add(total_mmap_size, Ordering::Relaxed);

        // If allocation is huge, try to use huge pages: better performance on runtime
        if class >= 9 {
            madvise(chunk, total_mmap_size, libc::MADV_HUGEPAGE);
        }

        // Build the free list - prev will be the HEAD of our new chain
        let mut prev: *mut Header = std::ptr::null_mut();

        // Build chain in reverse order
        for i in (0..ITERATIONS[class]).rev() {
            let current_header = (chunk as usize + i * block_size) as *mut Header;
            (*current_header).next = prev;
            (*current_header).size = SIZE_CLASSES[class] as u64;
            (*current_header).magic = MAGIC;
            prev = current_header;
        }

        // Now prev points to the HEAD (first block)
        // We need to find the TAIL to link it to the existing list
        let head = prev;
        let mut tail = prev;
        for _ in 0..ITERATIONS[class] - 1 {
            tail = (*tail).next;
        }

        let map = get_thread_cache();
        // Now link the TAIL to the existing list
        let mut list = map.map_list[class].load(Ordering::Acquire);
        loop {
            (*tail).next = list;

            match map.map_list[class].compare_exchange(
                list,
                head,
                Ordering::Release,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(stale_head) => {
                    list = stale_head;
                }
            }
        }
    }
}

fn pop_from_list(class: usize, cache: &ThreadLocalCache) -> *mut c_void {
    unsafe {
        loop {
            // Acquire the lock for the list
            let header = cache.map_list[class].load(Ordering::Acquire);

            if header.is_null() {
                return null_mut();
            }

            // Next element in the list
            let next = (*header).next;

            // Try to pop the element from the list
            if cache.map_list[class]
                .compare_exchange(header, next, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                return header as *mut c_void;
            }
        }
    }
}

// -----

static GLOBAL_LOCK: Mutex<()> = Mutex::new(());
static BOOT_STRAP: AtomicBool = AtomicBool::new(true);

fn bootstrap_once() {
    if !BOOT_STRAP.load(Ordering::Acquire) {
        return;
    }

    let _lock = GLOBAL_LOCK.lock().unwrap();
    if !BOOT_STRAP.load(Ordering::Relaxed) {
        return;
    }

    get_thread_cache();
    BOOT_STRAP.store(false, Ordering::Release);
}

// -----

fn maybe_trim() {
    let used = TOTAL_USED.load(Ordering::Relaxed);
    let allocated = TOTAL_ALLOCATED.load(Ordering::Relaxed);

    if allocated == 0 {
        return;
    }

    let usage_percent = (used * 100) / allocated;

    if usage_percent > 50 {
        return;
    }

    trim_unused_blocks();
}

fn trim_unused_blocks() {
    let cache = get_thread_cache();

    for class in (0..20).rev() {
        let mut count = 0;
        let mut node = cache.map_list[class].load(Ordering::Acquire);

        while !node.is_null() && count < 100 {
            count += 1;
            unsafe {
                node = (*node).next;
            }
        }

        if count > ITERATIONS[class] {
            let blocks_to_trim = count / 2;
            trim_blocks(class, blocks_to_trim);
        }
    }

    for class in (0..20).rev() {
        let mut count = 0;
        let mut node = GLOBAL_MAP_LIST[class].load(Ordering::Acquire);
        while !node.is_null() && count < 100 {
            count += 1;
            unsafe {
                node = (*node).next;
            }
        }
        if count > ITERATIONS[class] {
            let blocks_to_trim = count / 2;
            trim_blocks_global(class, blocks_to_trim);
        }
    }
}

fn trim_blocks_global(class: usize, count: usize) {
    for _ in 0..count {
        unsafe {
            let block = pop_batch_from_global(class, 1);
            if block.is_null() {
                break;
            }
            let size = SIZE_CLASSES[class] + HEADER_SIZE;
            madvise(block as *mut c_void, size, MADV_DONTNEED);
            TOTAL_ALLOCATED.fetch_sub(size, Ordering::Relaxed);
        }
    }
}

fn trim_blocks(class: usize, count: usize) {
    for _ in 0..count {
        unsafe {
            let block = pop_from_list(class, get_thread_cache());
            if block.is_null() {
                break;
            }
            let size = SIZE_CLASSES[class] + HEADER_SIZE;
            madvise(block, size, MADV_DONTNEED);
            TOTAL_ALLOCATED.fetch_sub(size, Ordering::Relaxed);
        }
    }
}

// -----

#[unsafe(no_mangle)]
pub extern "C" fn malloc(size: size_t) -> *mut c_void {
    bootstrap_once();

    let class = match match_size_class(size) {
        Some(size) => size,
        None => return big_alloc(size),
    };

    // Search the free list for a suitable block
    let map = get_thread_cache();

    let mut header_ptr = pop_from_list(class, map);

    if header_ptr.is_null() {
        // Checks if the global list has allocated pages
        let batch = unsafe { pop_batch_from_global(class, 16) };
        if !batch.is_null() {
            map.map_list[class].store(batch, Ordering::Release);
            header_ptr = pop_from_list(class, map);
        }
    }

    // If the free list is empty, try to allocate a new block
    if header_ptr.is_null() {
        for _ in 0..3 {
            // Trying to allocate a new block
            if bulk_allocate(class) {
                let ptr = pop_from_list(class, map);
                // Assigns the pointer to the header pointer
                header_ptr = ptr;
                break;
            }
        }
    }

    if header_ptr.is_null() {
        return null_mut();
    }

    unsafe {
        let header_ptr = header_ptr as *mut Header;
        TOTAL_USED.fetch_add(SIZE_CLASSES[class], Ordering::Relaxed);

        // header_ptr + HEADER_SIZE = actual pointer to the allocated block
        (header_ptr as *mut u8).add(HEADER_SIZE) as *mut c_void
    }
}

// -----

#[inline(always)]
fn is_our_pointer(ptr: *mut c_void) -> bool {
    let addr = ptr as usize;
    addr >= OUR_VA_START && addr < OUR_VA_END
}

#[unsafe(no_mangle)]
pub extern "C" fn free(ptr: *mut c_void) {
    if ptr.is_null() || BOOT_STRAP.load(Ordering::Relaxed) {
        return;
    }

    if !is_our_pointer(ptr) {
        return;
    }

    let header_addr = (ptr as usize).saturating_sub(HEADER_SIZE);
    if header_addr >= (ptr as usize) {
        return;
    }

    let header = header_addr as *mut Header;

    // Check if the header is valid
    if unsafe { (*header).magic } != MAGIC {
        return;
    }

    let size = unsafe { (*header).size };
    let total_size = HEADER_SIZE + size as usize;

    // Check if the header size is valid, if not than handle big-pages via munmap
    let class = match match_size_class(size as usize) {
        Some(class) => class,
        None => unsafe {
            // FIXME: better page handling
            if munmap(header as *mut c_void, total_size) != 0 {
                (*header).magic = 0;
                madvise(header as *mut c_void, total_size, MADV_DONTNEED);
            }
            return;
        },
    };

    let map = get_thread_cache();
    // # SAFETY:
    // 1- There shouldnt be any Infinite loops
    // 2- Data is writing back to the MapList
    unsafe {
        loop {
            // Acquire the head of the list
            let head = map.map_list[class].load(Ordering::Acquire);
            (*header).next = head;

            // If the CAS operation fails, try again
            if map.map_list[class]
                .compare_exchange(head, header, Ordering::Release, Ordering::Acquire)
                .is_ok()
            {
                TOTAL_USED.fetch_sub(size as usize, Ordering::Relaxed);
                break;
            }
        }
    }

    if TRIM_COUNTER.fetch_add(1, Ordering::Relaxed) % 5000 == 0 {
        maybe_trim();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn realloc(ptr: *mut c_void, new_size: size_t) -> *mut c_void {
    // Case 1: NULL pointer = just malloc
    if ptr.is_null() {
        return malloc(new_size);
    }

    // Case 2: Zero size = free and return minimal allocation
    if new_size == 0 {
        free(ptr);
        return malloc(1);
    }

    unsafe {
        // Try to get our header
        let header = (ptr as *mut u8).sub(HEADER_SIZE) as *mut Header;

        if !(*header).magic == MAGIC {
            return null_mut();
        }

        // It's our allocation
        let old_size = (*header).size as usize;

        let old_class = match_size_class(old_size);
        let new_class = match_size_class(new_size);

        if new_class == old_class {
            return ptr;
        }

        // Need new allocation
        let new_ptr = malloc(new_size);
        if new_ptr.is_null() {
            return std::ptr::null_mut();
        }

        if old_class.is_none() && new_class.is_none() {
            let old_total = old_size + HEADER_SIZE;
            let new_total = new_size + HEADER_SIZE;

            let result = libc::mremap(
                header as *mut c_void,
                old_total,
                new_total,
                libc::MREMAP_MAYMOVE,
                std::ptr::null_mut::<c_void>(),
            );

            if result != libc::MAP_FAILED {
                let new_header = result as *mut Header;
                (*new_header).size = new_size as u64;
                return (new_header as *mut u8).add(HEADER_SIZE) as *mut c_void;
            }
        }

        // Copy old data
        std::ptr::copy_nonoverlapping(ptr as *const u8, new_ptr as *mut u8, old_size.min(new_size));

        // Free old allocation
        free(ptr);
        new_ptr
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn calloc(nmemb: size_t, size: size_t) -> *mut c_void {
    let total_size = match nmemb.checked_mul(size) {
        Some(s) => s,
        None => return std::ptr::null_mut(),
    };

    if total_size == 0 {
        return std::ptr::null_mut();
    }

    let ptr = malloc(total_size);

    if !ptr.is_null() {
        unsafe {
            std::ptr::write_bytes(ptr as *mut u8, 0, total_size);
        }
    }

    ptr
}

// ---

#[test]
fn bench_allocator() {
    use std::{hint::black_box, time::Instant};

    let iterations = 1_000_000;

    // Warm up
    for _ in black_box(0..1000) {
        let ptr = black_box(malloc(100));
        black_box(free(ptr));
    }

    // Bench small allocations
    let start = Instant::now();
    for _ in black_box(0..iterations) {
        let ptr = black_box(malloc(100));
        black_box(free(ptr));
    }
    let small_time = start.elapsed();
    println!(
        "Small (100B): {:?} ({:.2} ns/op)",
        small_time,
        small_time.as_nanos() as f64 / iterations as f64
    );

    // Bench medium allocations
    let start = Instant::now();
    for _ in black_box(0..iterations) {
        let ptr = black_box(malloc(8192));
        black_box(free(ptr));
    }
    let med_time = start.elapsed();
    println!(
        "Medium (8KB): {:?} ({:.2} ns/op)",
        med_time,
        med_time.as_nanos() as f64 / iterations as f64
    );

    // Bench large allocations
    let start = Instant::now();
    for _ in black_box(0..10000) {
        let ptr = black_box(malloc(1024 * 1024 * 2));
        black_box(free(ptr));
    }
    let large_time = start.elapsed();
    println!(
        "Large (1MB): {:?} ({:.2} ns/op)",
        large_time,
        large_time.as_nanos() as f64 / 10000.0
    );
}

// --------------------------------------------
