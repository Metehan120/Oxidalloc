use core::{
    fmt::{self, Write},
    mem::size_of,
};

use crate::internals::oncelock::OnceLock;
use crate::sys::{
    memory_system::{MMapFlags, MProtFlags, MemoryFlags, mmap_memory},
    syscall_linux::{close_fd, openat_ro, read_fd},
};

use crate::sys::memory_system::CPU_AFFINITY_WORDS;

pub const MAX_CPUS: usize = CPU_AFFINITY_WORDS * 64;
const MAX_NODE_PATH: usize = 128;
const CPULIST_BUF: usize = 4096;

pub struct NumaMaps {
    pub node_offsets: *mut usize,
    pub node_cpus: *mut usize,
    pub cpu_to_node: *mut usize,
    pub node_count: usize,
    pub node_cpu_count: usize,
}

static NUMA_MAPS: OnceLock<Option<NumaMaps>> = OnceLock::new();

struct StackPath<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> Write for StackPath<'a> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        if self.len + bytes.len() > self.buf.len() {
            return Err(fmt::Error);
        }
        self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }
}

fn write_node_path(buf: &mut [u8; MAX_NODE_PATH], node: usize) -> Option<usize> {
    let mut out = StackPath { buf, len: 0 };
    write!(&mut out, "/sys/devices/system/node/node{}/cpulist", node).ok()?;
    if out.len + 1 > out.buf.len() {
        return None;
    }
    out.buf[out.len] = 0;
    Some(out.len)
}

fn parse_cpulist_bytes(
    bytes: &[u8],
    node_index: usize,
    node_cpus: &mut [usize],
    cpu_to_node: &mut [usize],
    node_cpu_count: &mut usize,
) {
    let mut i = 0usize;
    let len = bytes.len();

    let parse_num = |i: &mut usize| -> Option<usize> {
        while *i < len && (bytes[*i] == b' ' || bytes[*i] == b'\n' || bytes[*i] == b'\t') {
            *i += 1;
        }
        let mut val = 0usize;
        let mut found = false;
        while *i < len && bytes[*i].is_ascii_digit() {
            found = true;
            val = val * 10 + (bytes[*i] - b'0') as usize;
            *i += 1;
        }
        if found { Some(val) } else { None }
    };

    while i < len {
        let start = match parse_num(&mut i) {
            Some(v) => v,
            None => break,
        };

        let end = if i < len && bytes[i] == b'-' {
            i += 1;
            parse_num(&mut i).unwrap_or(start)
        } else {
            start
        };

        let end = end.max(start);
        for cpu in start..=end {
            if cpu >= MAX_CPUS {
                continue;
            }
            if *node_cpu_count >= node_cpus.len() {
                break;
            }
            node_cpus[*node_cpu_count] = cpu;
            if cpu < cpu_to_node.len() {
                cpu_to_node[cpu] = node_index;
            }
            *node_cpu_count += 1;
        }

        while i < len && bytes[i] != b',' {
            i += 1;
        }
        if i < len && bytes[i] == b',' {
            i += 1;
        }
    }
}

fn build_numa_maps(max_nodes: usize) -> Option<NumaMaps> {
    let node_offsets = unsafe {
        mmap_memory(
            core::ptr::null_mut(),
            size_of::<usize>() * (max_nodes + 1),
            MMapFlags {
                prot: MProtFlags::READ | MProtFlags::WRITE,
                map: MemoryFlags::PRIVATE,
            },
        )
        .ok()?
    } as *mut usize;

    let node_cpus = unsafe {
        mmap_memory(
            core::ptr::null_mut(),
            size_of::<usize>() * MAX_CPUS,
            MMapFlags {
                prot: MProtFlags::READ | MProtFlags::WRITE,
                map: MemoryFlags::PRIVATE,
            },
        )
        .ok()?
    } as *mut usize;

    let cpu_to_node = unsafe {
        mmap_memory(
            core::ptr::null_mut(),
            size_of::<usize>() * MAX_CPUS,
            MMapFlags {
                prot: MProtFlags::READ | MProtFlags::WRITE,
                map: MemoryFlags::PRIVATE,
            },
        )
        .ok()?
    } as *mut usize;

    let offsets = unsafe { core::slice::from_raw_parts_mut(node_offsets, max_nodes + 1) };
    let cpus = unsafe { core::slice::from_raw_parts_mut(node_cpus, MAX_CPUS) };
    let cpu_map = unsafe { core::slice::from_raw_parts_mut(cpu_to_node, MAX_CPUS) };

    for slot in cpu_map.iter_mut() {
        *slot = usize::MAX;
    }

    let mut node_count = 0usize;
    let mut node_cpu_count = 0usize;

    for node in 0..max_nodes {
        let mut path = [0u8; MAX_NODE_PATH];
        let Some(_len) = write_node_path(&mut path, node) else {
            break;
        };

        let fd = unsafe { openat_ro(path.as_ptr()) };
        let Ok(fd) = fd else {
            continue;
        };

        let mut buf = [0u8; CPULIST_BUF];
        let mut read_len = 0usize;
        if let Ok(n) = unsafe { read_fd(fd, &mut buf) } {
            read_len = n;
        }
        let _ = unsafe { close_fd(fd) };

        if read_len == 0 {
            continue;
        }

        offsets[node_count] = node_cpu_count;
        parse_cpulist_bytes(
            &buf[..read_len],
            node_count,
            cpus,
            cpu_map,
            &mut node_cpu_count,
        );
        node_count += 1;
        if node_count == max_nodes {
            break;
        }
    }

    if node_count == 0 {
        return None;
    }

    offsets[node_count] = node_cpu_count;

    Some(NumaMaps {
        node_offsets,
        node_cpus,
        cpu_to_node,
        node_count,
        node_cpu_count,
    })
}

pub fn get_numa_maps(max_nodes: usize) -> Option<&'static NumaMaps> {
    NUMA_MAPS
        .get_or_init(|| build_numa_maps(max_nodes))
        .as_ref()
}
