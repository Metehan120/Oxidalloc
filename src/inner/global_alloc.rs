use std::alloc::{GlobalAlloc, Layout};
use std::os::raw::c_void;

use crate::inner::{
    alloc::alloc_inner, calloc::calloc_inner, free::free_inner, memalign::memalign_inner,
    realloc::realloc_inner,
};

pub struct Oxidalloc;

unsafe impl GlobalAlloc for Oxidalloc {
    #[inline(always)]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() <= 16 {
            alloc_inner(layout.size()) as *mut u8
        } else {
            memalign_inner(layout.align(), layout.size()) as *mut u8
        }
    }

    #[inline(always)]
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        free_inner(ptr as *mut c_void);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if layout.align() <= 16 {
            realloc_inner(ptr as *mut c_void, new_size) as *mut u8
        } else {
            let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
            let new_ptr = self.alloc(new_layout);
            if !new_ptr.is_null() {
                std::ptr::copy_nonoverlapping(ptr, new_ptr, layout.size().min(new_size));
                self.dealloc(ptr, layout);
            }
            new_ptr
        }
    }

    #[inline(always)]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if layout.align() <= 16 {
            calloc_inner(1, layout.size()) as *mut u8
        } else {
            let ptr = self.alloc(layout);
            if !ptr.is_null() {
                std::ptr::write_bytes(ptr, 0, layout.size());
            }
            ptr
        }
    }
}
