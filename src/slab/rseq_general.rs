use core::arch::asm;
use std::arch::global_asm;

use crate::OxHeader;

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
    static __rseq_offset: isize;
    static __rseq_size: u32;
}

pub unsafe fn get_rseq() -> &'static rseq {
    let rseq_ptr: *mut rseq;
    let _: usize;

    #[cfg(target_arch = "x86_64")]
    asm!(
        "mov {tmp}, qword ptr [{offset_sym}@GOTPCREL + rip]",
        "mov {offset}, [{tmp}]",
        "mov {rseq_ptr}, qword ptr fs:[{offset}]",

        tmp = out(reg) _,
        offset = out(reg) _,
        offset_sym = sym __rseq_offset,
        rseq_ptr = out(reg) rseq_ptr,
        options(readonly, nostack, preserves_flags)
    );

    #[cfg(target_arch = "aarch64")]
    asm!(
        "adrp {tmp}, :gottprel:__rseq_offset",
        "ldr {offset}, [{tmp}, :gottprel_lo12:__rseq_offset]",
        "mrs {tp}, tpidr_el0",
        "add {rseq_ptr}, {tp}, {offset}",

        tmp = out(reg) _,
        offset = out(reg) _,
        tp = out(reg) _,
        rseq_ptr = out(reg) rseq_ptr,
        options(readonly, nostack, preserves_flags)
    );

    &*rseq_ptr
}

#[cfg(target_arch = "x86_64")]
global_asm!(include_str!("asm/rseq_pop_x86.s"));
#[cfg(target_arch = "x86_64")]
global_asm!(include_str!("asm/rseq_push_x86.s"));

unsafe extern "C" {
    // RDI: head_ptr
    // RSI: rseq_cs_ptr (TLS)
    // RDX: counter_ptr
    pub fn rseq_pop_header(
        head_ptr: *mut *mut OxHeader,
        rseq_cs_ptr: *mut usize,
        counter_ptr: *mut usize,
    ) -> *mut OxHeader;

    // RDI: head_ptr
    // RSI: new_node
    // RDX: rseq_cs_ptr (TLS)
    // RCX: counter_ptr
    pub fn rseq_push_header(
        head_ptr: *mut *mut OxHeader,
        new_node: *mut OxHeader,
        rseq_cs_ptr: *mut usize,
        counter_ptr: *mut usize,
    );
}

#[inline(always)]
pub unsafe fn get_cs_ptr() -> *mut usize {
    let rseq_info = get_rseq();
    core::ptr::addr_of!((*rseq_info).rseq_cs) as *mut usize
}
