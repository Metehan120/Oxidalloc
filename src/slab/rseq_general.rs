use core::arch::asm;
use std::os::raw::c_char;

#[repr(C, align(32))]
pub struct rseq {
    pub cpu_id: u32,
    pub cpu_id_start: u32,
    pub rseq_cs: u64,
    pub flags: u32,
    pub node_id: u32,
    pub mm_cid: u32,
}

pub const RSEQ_SIG: u32 = 0x53053053;

unsafe extern "C" {
    fn gnu_get_libc_version() -> *const c_char;
    static __rseq_offset: isize;
    static __rseq_size: u32;
}

pub unsafe fn is_glibc_new_enough() -> bool {
    let version_ptr = gnu_get_libc_version();
    if version_ptr.is_null() {
        return false;
    }

    let mut s = version_ptr as *const u8;
    let mut major = 0u32;
    while *s >= b'0' && *s <= b'9' {
        major = major * 10 + (*s - b'0') as u32;
        s = s.add(1);
    }
    if *s != b'.' {
        return major > 2;
    }
    s = s.add(1);
    let mut minor = 0u32;
    while *s >= b'0' && *s <= b'9' {
        minor = minor * 10 + (*s - b'0') as u32;
        s = s.add(1);
    }
    major > 2 || (major == 2 && minor >= 35)
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn get_cpu_id() -> u32 {
    let cpu_id: u32;
    asm!(
        "mov {offset}, qword ptr [{offset_sym}@GOTPCREL + rip]",
        "mov {offset}, [{offset}]",
        "mov {cpu_id:e}, fs:[{offset} + 4]",
        offset = out(reg) _,
        offset_sym = sym __rseq_offset,
        cpu_id = out(reg) cpu_id,
        options(readonly, nostack, preserves_flags)
    );
    cpu_id
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub unsafe fn get_cpu_id() -> u32 {
    let cpu_id: u32;
    let mut offset: isize;
    asm!(
        "mrs {tp}, tpidr_el0",
        "adrp {tmp}, :got:__rseq_offset",
        "ldr {tmp}, [{tmp}, :got_lo12:__rseq_offset]",
        "ldr {offset}, [{tmp}]",
        "add {tp}, {tp}, {offset}",
        "ldr {cpu_id:w}, [{tp}, #4]",
        tp = out(reg) _,
        tmp = out(reg) _,
        offset = out(reg) offset,
        cpu_id = out(reg) cpu_id,
        options(readonly, nostack, preserves_flags)
    );
    cpu_id
}
