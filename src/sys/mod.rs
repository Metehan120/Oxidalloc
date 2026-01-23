#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "macos"))]
mod syscall_unix;

#[allow(unused)]
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "macos"))]
mod unix {
    use rustix::{
        io::Errno,
        mm::{Advice, MapFlags, MprotectFlags, ProtFlags},
        rand::GetRandomFlags,
    };
    use std::ops::BitOr;

    pub const EINVAL: i32 = Errno::INVAL.raw_os_error();
    pub const NOMEM: i32 = Errno::NOMEM.raw_os_error();

    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    #[repr(i32)]
    pub enum SysErr {
        OOM = NOMEM,
        Unaligned = EINVAL,
        RandomReqFail = 0,
        Other = 1,
    }

    impl SysErr {
        pub fn get_errno(&self) -> i32 {
            match self {
                SysErr::OOM => NOMEM,
                SysErr::Unaligned => EINVAL,
                SysErr::RandomReqFail => 1,
                SysErr::Other => 0,
            }
        }
    }

    pub struct MemoryFlags(pub MapFlags);
    impl MemoryFlags {
        pub const PRIVATE: MemoryFlags = MemoryFlags(MapFlags::PRIVATE);
        pub const FIXED: MemoryFlags = MemoryFlags(MapFlags::FIXED);
        pub const NORESERVE: MemoryFlags = MemoryFlags(MapFlags::NORESERVE);
    }

    impl BitOr for MemoryFlags {
        type Output = MemoryFlags;

        fn bitor(self, rhs: Self) -> Self::Output {
            MemoryFlags(self.0 | rhs.0)
        }
    }

    pub struct MProtFlags(pub ProtFlags);
    impl MProtFlags {
        pub const NONE: Self = MProtFlags(ProtFlags::empty());
        pub const READ: Self = MProtFlags(ProtFlags::READ);
        pub const WRITE: Self = MProtFlags(ProtFlags::WRITE);
    }
    impl BitOr for MProtFlags {
        type Output = MProtFlags;

        fn bitor(self, rhs: Self) -> Self::Output {
            MProtFlags(self.0 | rhs.0)
        }
    }

    pub struct RMProtFlags(pub MprotectFlags);
    impl RMProtFlags {
        pub const READ: RMProtFlags = RMProtFlags(MprotectFlags::READ);
        pub const WRITE: RMProtFlags = RMProtFlags(MprotectFlags::WRITE);
        pub const NONE: RMProtFlags = RMProtFlags(MprotectFlags::empty());
    }
    impl BitOr for RMProtFlags {
        type Output = RMProtFlags;

        fn bitor(self, rhs: Self) -> Self::Output {
            RMProtFlags(self.0 | rhs.0)
        }
    }

    pub struct RandomFlags(pub GetRandomFlags);
    impl RandomFlags {
        pub const NONBLOCK: Self = RandomFlags(GetRandomFlags::NONBLOCK);
        pub const RANDOM: Self = RandomFlags(GetRandomFlags::RANDOM);
        pub const NONE: Self = RandomFlags(GetRandomFlags::empty());
    }
    impl BitOr for RandomFlags {
        type Output = RandomFlags;

        fn bitor(self, rhs: Self) -> Self::Output {
            RandomFlags(self.0 | rhs.0)
        }
    }

    pub struct MadviseFlags(pub Advice);
    impl MadviseFlags {
        pub const DONTNEED: Self = MadviseFlags(Advice::DontNeed);
    }

    pub struct MMapFlags {
        pub prot: MProtFlags,
        pub map: MemoryFlags,
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use crate::sys::unix::MProtFlags;
    use rustix::mm::{Advice, MapFlags};
    use std::ops::BitOr;

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
        pub const HUGEPAGE: Self = MadviseFlags(Advice::LinuxHugepage);
        pub const DONTDUMP: Self = MadviseFlags(Advice::LinuxDontDump);
        pub const DONTNEED: Self = MadviseFlags(Advice::LinuxDontNeed);
    }

    pub struct MMapFlags {
        pub prot: MProtFlags,
        pub map: MemoryFlags,
    }
}

#[cfg(target_os = "linux")]
pub mod memory_system {
    pub use crate::sys::linux::{MMapFlags, MadviseFlags, MemoryFlags};
    use crate::sys::syscall_unix::{
        get_random_val, madvise_memory, map_memory, mprotect_memory, munmap_memory,
    };
    pub use crate::sys::unix::{MProtFlags, RMProtFlags, RandomFlags, SysErr};
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

    pub unsafe fn get_random(buf: &mut [u8], flags: RandomFlags) -> Result<usize, SysErr> {
        get_random_val(buf, flags.0)
    }
}
