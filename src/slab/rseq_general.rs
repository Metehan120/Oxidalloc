use core::arch::asm;
use std::os::raw::c_char;

unsafe extern "C" {
    fn gnu_get_libc_version() -> *const c_char;
}

unsafe extern "C" {
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
