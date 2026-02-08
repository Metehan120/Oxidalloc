use std::alloc::{GlobalAlloc, Layout};
use std::hint::likely;
use std::os::raw::c_void;

use crate::inner::{
    alloc::alloc_inner, calloc::calloc_inner, free::free_inner, memalign::memalign_inner,
    realloc::realloc_inner,
};
use crate::internals::once::Once;
use crate::{OX_DISABLE_THP, OX_FORCE_THP, OX_MAX_RESERVATION, OX_TRIM, OX_TRIM_THRESHOLD};

pub struct Oxidalloc {
    config: OxidallocConfig,
    once: Once,
}

pub struct OxidallocConfig {
    pub disable_trim: bool,
    pub disable_thp: bool,
    pub force_thp: bool,
    trim_threshold: usize,
    max_reservation: usize,
}

impl OxidallocConfig {
    pub const fn new() -> Self {
        Self {
            disable_trim: false,
            disable_thp: false,
            force_thp: false,
            trim_threshold: 1024 * 1024 * 10,
            max_reservation: 1024 * 1024 * 1024 * 16,
        }
    }

    pub const fn change_trim_threshold(&mut self, mut new_threshold: usize) {
        if new_threshold == 0 || new_threshold < 1024 * 1024 {
            new_threshold = 1024 * 1024;
        }

        self.trim_threshold = new_threshold;
    }

    pub const fn change_max_reservation(&mut self, new_max_reservation: usize) {
        let mut next_power_of_two = match new_max_reservation.checked_next_power_of_two() {
            Some(out) => out,
            None => 1024 * 1024 * 1024 * 16,
        };

        if next_power_of_two >= 1024 * 1024 * 1024 * 1024 * 256 {
            next_power_of_two = 1024 * 1024 * 1024 * 1024 * 256;
        }
        if next_power_of_two <= 1024 * 1024 * 1024 * 16 {
            next_power_of_two = 1024 * 1024 * 1024 * 16;
        }

        self.max_reservation = next_power_of_two;
    }
}

impl Oxidalloc {
    pub const fn new_with_config(config: OxidallocConfig) -> Self {
        Self {
            config,
            once: Once::new(),
        }
    }

    pub const fn new() -> Self {
        Self::new_with_config(OxidallocConfig::new())
    }

    unsafe fn init(&self) {
        self.once.call_once(|| {
            OX_TRIM = !self.config.disable_trim;
            OX_DISABLE_THP = self.config.disable_thp;
            OX_FORCE_THP = self.config.force_thp;
            OX_MAX_RESERVATION = self.config.max_reservation;
            OX_TRIM_THRESHOLD = self.config.trim_threshold;
        });
    }
}

unsafe impl GlobalAlloc for Oxidalloc {
    #[inline(always)]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.init();

        if likely(layout.align() <= 16) {
            alloc_inner(layout.size()) as *mut u8
        } else {
            memalign_inner(layout.align(), layout.size()) as *mut u8
        }
    }

    #[inline(always)]
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        self.init();

        free_inner(ptr as *mut c_void);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        self.init();

        if likely(layout.align() <= 16) {
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
        self.init();

        if likely(layout.align() <= 16) {
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
