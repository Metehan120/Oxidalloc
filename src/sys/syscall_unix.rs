use std::os::raw::c_void;

use rustix::{
    mm::{Advice, MapFlags, MprotectFlags, ProtFlags, madvise, mmap_anonymous, mprotect, munmap},
    rand::{GetRandomFlags, getrandom},
};

use crate::sys::unix::{EINVAL, NOMEM, SysErr};

pub unsafe fn map_memory(
    ptr: *mut c_void,
    len: usize,
    prot: ProtFlags,
    map_flags: MapFlags,
) -> Result<*mut c_void, SysErr> {
    match mmap_anonymous(ptr, len, prot, map_flags) {
        Ok(mapped_ptr) => Ok(mapped_ptr),
        Err(e) => match e.raw_os_error() {
            NOMEM => Err(SysErr::OOM),
            EINVAL => Err(SysErr::Unaligned),
            _ => Err(SysErr::Other),
        },
    }
}

pub unsafe fn munmap_memory(ptr: *mut c_void, len: usize) -> Result<(), SysErr> {
    match munmap(ptr, len) {
        Ok(_) => Ok(()),
        Err(e) => match e.raw_os_error() {
            NOMEM => Err(SysErr::OOM),
            EINVAL => Err(SysErr::Unaligned),
            _ => Err(SysErr::Other),
        },
    }
}

pub unsafe fn madvise_memory(
    ptr: *mut c_void,
    len: usize,
    mprot_flags: Advice,
) -> Result<(), SysErr> {
    match madvise(ptr, len, mprot_flags) {
        Ok(_) => Ok(()),
        Err(e) => match e.raw_os_error() {
            EINVAL => Err(SysErr::Unaligned),
            _ => Err(SysErr::Other),
        },
    }
}

pub unsafe fn mprotect_memory(
    ptr: *mut c_void,
    len: usize,
    mprot_flags: MprotectFlags,
) -> Result<(), SysErr> {
    match mprotect(ptr, len, mprot_flags) {
        Ok(_) => Ok(()),
        Err(e) => match e.raw_os_error() {
            EINVAL => Err(SysErr::Unaligned),
            _ => Err(SysErr::Other),
        },
    }
}

pub unsafe fn get_random_val(val: &mut [u8], flags: GetRandomFlags) -> Result<usize, SysErr> {
    getrandom(val, flags).map_err(|_| SysErr::RandomReqFail)
}
