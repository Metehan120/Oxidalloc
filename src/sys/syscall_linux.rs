// ⚠️ WARNING ⚠️
// This file intentionally talks directly to the Linux kernel.
// It exists to provide the smallest possible abstraction layer
// with full control over allocator behavior.
//
// This is not a general-purpose syscall wrapper.
// If you want safety, portability, or convenience, use libc/rustix.
// This file is part of the allocator's trusted computing base.

use std::os::raw::c_void;

use core::arch::asm;

use crate::sys::{EEXIST, EINVAL, NOMEM, linux::SysErr};

#[repr(transparent)]
#[derive(Copy, Clone)]
pub struct Advice(usize);

impl Advice {
    pub const DONTNEED: Self = Self(4);
    pub const HUGEPAGE: Self = Self(14);
    pub const NORMAL: Self = Self(0);
}

#[repr(transparent)]
#[derive(Debug, Copy, Clone)]
pub struct ProtFlags(usize);

impl ProtFlags {
    pub const NONE: Self = Self(0);
    pub const READ: Self = Self(1);
    pub const WRITE: Self = Self(2);
}

impl core::ops::BitOr for ProtFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

#[repr(transparent)]
#[derive(Debug, Copy, Clone)]
pub struct MapFlags(usize);

impl MapFlags {
    pub const PRIVATE: Self = Self(2);
    pub const FIXED: Self = Self(16);
    pub const NORESERVE: Self = Self(16384);
    pub const FIXED_NOREPLACE: Self = Self(1048576);
    pub const ANONYMOUS: Self = Self(32);
}

impl core::ops::BitOr for MapFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

pub struct Sys;

#[cfg(target_arch = "x86_64")]
impl Sys {
    const SYS_MMAP: usize = 9;
    const SYS_MUNMAP: usize = 11;
    const SYS_MADVISE: usize = 28;
    const SYS_MPROTECT: usize = 10;
    const SYS_GETRANDOM: usize = 318;
    const SYS_RSEQ: usize = 334;
}

#[cfg(target_arch = "aarch64")]
impl Sys {
    const SYS_MMAP: usize = 222;
    const SYS_MUNMAP: usize = 215;
    const SYS_MADVISE: usize = 233;
    const SYS_MPROTECT: usize = 226;
    const SYS_GETRANDOM: usize = 278;
    const SYS_RSEQ: usize = 293;
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn syscall6(
    n: usize,
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
    a6: usize,
) -> isize {
    let ret: isize;
    asm!(
        "syscall",
        inlateout("rax") n as isize => ret,
        in("rdi") a1,
        in("rsi") a2,
        in("rdx") a3,
        in("r10") a4,
        in("r8")  a5,
        in("r9")  a6,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack),
    );
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn syscall6(
    n: usize,
    a0: usize,
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
) -> isize {
    let ret: isize;
    asm!(
        "svc #0",
        inlateout("x0") a0 as isize => ret,
        in("x1") a1,
        in("x2") a2,
        in("x3") a3,
        in("x4") a4,
        in("x5") a5,
        in("x8") n,
        lateout("x6") _,
        lateout("x7") _,
        options(nostack),
    );
    ret
}

#[inline(always)]
fn syscall_result(ret: isize) -> Result<usize, i32> {
    if ret < 0 {
        Err(-ret as i32)
    } else {
        Ok(ret as usize)
    }
}

pub unsafe fn map_memory(
    ptr: *mut c_void,
    len: usize,
    prot: ProtFlags,
    flags: MapFlags,
) -> Result<*mut c_void, SysErr> {
    let ret = syscall6(
        Sys::SYS_MMAP,
        ptr as usize,
        len,
        prot.0,
        (flags | MapFlags::ANONYMOUS).0,
        usize::MAX, // fd = -1
        0,          // offset
    );

    match syscall_result(ret) {
        Ok(out) => Ok(out as *mut c_void),
        Err(e) => match e {
            NOMEM => Err(SysErr::OOM),
            EINVAL => Err(SysErr::Unaligned),
            EEXIST => Err(SysErr::MemAlreadyMapped),
            _ => Err(SysErr::Other),
        },
    }
}

pub unsafe fn munmap_memory(ptr: *mut c_void, len: usize) -> Result<(), SysErr> {
    let ret = syscall6(Sys::SYS_MUNMAP, ptr as usize, len, 0, 0, 0, 0);

    match syscall_result(ret) {
        Ok(_) => Ok(()),
        Err(e) => match e {
            NOMEM => Err(SysErr::OOM),
            EINVAL => Err(SysErr::Unaligned),
            _ => Err(SysErr::Other),
        },
    }
}

pub unsafe fn madvise_memory(ptr: *mut c_void, len: usize, madvise: Advice) -> Result<(), SysErr> {
    let ret = syscall6(Sys::SYS_MADVISE, ptr as usize, len, madvise.0, 0, 0, 0);

    match syscall_result(ret) {
        Ok(_) => Ok(()),
        Err(EINVAL) => Err(SysErr::Unaligned),
        _ => Err(SysErr::Other),
    }
}

pub unsafe fn mprotect_memory(
    ptr: *mut c_void,
    len: usize,
    mprot_flags: ProtFlags,
) -> Result<(), SysErr> {
    let ret = syscall6(Sys::SYS_MPROTECT, ptr as usize, len, mprot_flags.0, 0, 0, 0);

    match syscall_result(ret) {
        Ok(_) => Ok(()),
        Err(EINVAL) => Err(SysErr::Unaligned),
        _ => Err(SysErr::Other),
    }
}

pub unsafe fn get_random_val<T>(buf: &mut [T]) -> Result<usize, SysErr> {
    let ret = syscall6(
        Sys::SYS_GETRANDOM,
        buf.as_mut_ptr() as usize,
        buf.len() * size_of::<T>(),
        0,
        0,
        0,
        0,
    );

    syscall_result(ret).map_err(|_| SysErr::RandomReqFail)
}

#[thread_local]
static mut RSEQ_STATE: i8 = 0;

#[inline(always)]
pub unsafe fn register_rseq(ptr: *mut c_void, len: usize, sig: u32) -> Result<(), i32> {
    if RSEQ_STATE != 0 {
        return if RSEQ_STATE == 1 { Ok(()) } else { Err(-1) };
    }

    let ret = syscall6(Sys::SYS_RSEQ, ptr as usize, len, 0, sig as usize, 0, 0);

    if ret < 0 {
        RSEQ_STATE = -1;
        Err(-ret as i32)
    } else {
        RSEQ_STATE = 1;
        Ok(())
    }
}
