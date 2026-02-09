use crate::{OxHeader, slab::interconnect::ICC};

// ----------------------------------

#[cfg(feature = "hardened-linked-list")]
pub(crate) unsafe fn reset_global_locks() {
    ICC.reset_on_fork();
}

pub struct GlobalHandler;

impl GlobalHandler {
    pub unsafe fn push_to_global(
        &self,
        class: usize,
        head: *mut OxHeader,
        tail: *mut OxHeader,
        batch_size: usize,
        need_push_pushed: bool,
        is_trimmed: bool,
    ) {
        #[cfg(feature = "hardened-linked-list")]
        {
            let mut curr = head;
            while curr != tail {
                use crate::{slab::xor_ptr_general, va::bootstrap::NUMA_KEY};

                let next_raw = (*curr).next;
                (*curr).next = xor_ptr_general(next_raw, NUMA_KEY);
                curr = next_raw;
            }
        }

        ICC.try_push(class, head, tail, batch_size, need_push_pushed, is_trimmed);
    }

    pub unsafe fn pop_from_global(
        &self,
        class: usize,
        batch_size: usize,
        need_pushed: bool,
    ) -> *mut OxHeader {
        ICC.try_pop(class, batch_size, need_pushed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hint::black_box;
    use std::ptr::null_mut;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Instant;

    #[test]
    fn test_global_speed_under_contention() {
        let num_threads = 12;
        let ops_per_thread = 50_000;
        let class = 10;
        let batch_size = 32;

        let barrier = Arc::new(Barrier::new(num_threads));
        let mut handles = Vec::new();

        let start_time = Instant::now();

        for _ in 0..num_threads {
            let barrier = Arc::clone(&barrier);

            handles.push(thread::spawn(move || {
                let mut headers = vec![
                    OxHeader {
                        next: null_mut(),
                        class: class as u8,
                        magic: 0x42,
                        life_time: 0,
                    };
                    batch_size * 2
                ];

                barrier.wait();

                for _ in 0..ops_per_thread {
                    let head = &mut headers[0] as *mut OxHeader;
                    let tail = &mut headers[batch_size - 1] as *mut OxHeader;

                    unsafe {
                        black_box(
                            GlobalHandler
                                .push_to_global(class, head, tail, batch_size, false, false),
                        );

                        let res =
                            black_box(GlobalHandler.pop_from_global(class, batch_size, false));

                        std::hint::black_box(res);
                    }
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let duration = start_time.elapsed();
        let total_ops = num_threads * ops_per_thread * 2;
        println!(
            "\nTotal Atomic Ops: {}\nTime: {:?}\nAvg Latency: {:.2} ns/op",
            total_ops,
            duration,
            duration.as_nanos() as f64 / total_ops as f64
        );
    }
}
