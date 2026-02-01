#[cfg(any(target_os = "linux"))]
mod syscall_linux;

pub const EINVAL: i32 = 22;
pub const NOMEM: i32 = 12;
pub const EEXIST: i32 = 17;

#[cfg(target_os = "linux")]
mod linux {

    use crate::sys::{
        EEXIST, EINVAL, NOMEM,
        syscall_linux::{Advice, MapFlags, ProtFlags},
    };
    use std::ops::BitOr;

    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    #[repr(i32)]
    pub enum SysErr {
        OOM = NOMEM,
        Unaligned = EINVAL,
        RandomReqFail = 0,
        MemAlreadyMapped = EEXIST,
        Other = 1,
    }

    impl SysErr {
        pub fn get_errno(&self) -> i32 {
            match self {
                SysErr::OOM => NOMEM,
                SysErr::Unaligned => EINVAL,
                SysErr::RandomReqFail => 1,
                SysErr::MemAlreadyMapped => EEXIST,
                SysErr::Other => 0,
            }
        }
    }

    #[derive(Debug, Clone)]
    pub struct MemoryFlags(pub MapFlags);
    impl MemoryFlags {
        pub const PRIVATE: MemoryFlags = MemoryFlags(MapFlags::PRIVATE);
        pub const FIXED: MemoryFlags = MemoryFlags(MapFlags::FIXED);
        pub const NORESERVE: MemoryFlags = MemoryFlags(MapFlags::NORESERVE);
        pub const FIXED_NOREPLACE: MemoryFlags = MemoryFlags(MapFlags::FIXED_NOREPLACE);
    }
    impl BitOr for MemoryFlags {
        type Output = MemoryFlags;

        fn bitor(self, rhs: Self) -> Self::Output {
            MemoryFlags(self.0 | rhs.0)
        }
    }

    pub struct MadviseFlags(pub Advice);
    impl MadviseFlags {
        pub const HUGEPAGE: Self = MadviseFlags(Advice::HUGEPAGE);
        pub const DONTNEED: Self = MadviseFlags(Advice::DONTNEED);
        pub const NORMAL: Self = MadviseFlags(Advice::NORMAL);
    }

    pub struct RMProtFlags(pub ProtFlags);
    impl RMProtFlags {
        pub const READ: RMProtFlags = RMProtFlags(ProtFlags::READ);
        pub const WRITE: RMProtFlags = RMProtFlags(ProtFlags::WRITE);
        pub const NONE: RMProtFlags = RMProtFlags(ProtFlags::NONE);
    }
    impl BitOr for RMProtFlags {
        type Output = RMProtFlags;

        fn bitor(self, rhs: Self) -> Self::Output {
            RMProtFlags(self.0 | rhs.0)
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub struct MProtFlags(pub ProtFlags);
    impl MProtFlags {
        pub const NONE: Self = MProtFlags(ProtFlags::NONE);
        pub const READ: Self = MProtFlags(ProtFlags::READ);
        pub const WRITE: Self = MProtFlags(ProtFlags::WRITE);
    }
    impl BitOr for MProtFlags {
        type Output = MProtFlags;

        fn bitor(self, rhs: Self) -> Self::Output {
            MProtFlags(self.0 | rhs.0)
        }
    }

    #[derive(Debug, Clone)]
    pub struct MMapFlags {
        pub prot: MProtFlags,
        pub map: MemoryFlags,
    }
}

#[cfg(target_os = "linux")]
pub mod memory_system {
    pub use crate::sys::linux::{
        MMapFlags, MProtFlags, MadviseFlags, MemoryFlags, RMProtFlags, SysErr,
    };
    use crate::sys::syscall_linux::{
        get_random_val, madvise_memory, map_memory, mprotect_memory, munmap_memory, register_rseq,
        syscall6,
    };
    use std::os::raw::c_void;

    pub unsafe fn unmap_memory(ptr: *mut c_void, size: usize) -> Result<(), SysErr> {
        munmap_memory(ptr, size)
    }

    pub unsafe fn mmap_memory(
        ptr: *mut c_void,
        size: usize,
        flags: MMapFlags,
    ) -> Result<*mut c_void, SysErr> {
        let mapf = flags.map;
        let protf = flags.prot;

        map_memory(ptr, size, protf.0, mapf.0)
    }

    pub unsafe fn madvise(
        ptr: *mut c_void,
        len: usize,
        madvise: MadviseFlags,
    ) -> Result<(), SysErr> {
        madvise_memory(ptr, len, madvise.0)
    }

    pub unsafe fn protect_memory(
        ptr: *mut c_void,
        len: usize,
        prot: RMProtFlags,
    ) -> Result<(), SysErr> {
        mprotect_memory(ptr, len, prot.0)
    }

    pub unsafe fn getrandom<T>(buf: &mut [T]) -> Result<usize, SysErr> {
        get_random_val(buf)
    }

    pub unsafe fn reg_rseq(ptr: *mut c_void, len: usize, sig: u32) -> Result<(), i32> {
        register_rseq(ptr, len, sig)
    }

    pub unsafe fn get_cpu_count() -> usize {
        let mut mask = [0u64; 8192 / 8]; // Supports up to 8192 cores
        let ret = syscall6(
            204,
            0,
            size_of_val(&mask),
            mask.as_mut_ptr() as usize,
            0,
            0,
            0,
        );

        if ret < 0 {
            return 1;
        }

        mask.iter().map(|part| part.count_ones() as usize).sum()
    }
}
