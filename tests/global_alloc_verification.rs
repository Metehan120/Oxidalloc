#[cfg(feature = "global-alloc")]
pub mod tests {
    use oxidalloc::Oxidalloc;
    use std::alloc::{GlobalAlloc, Layout};

    #[global_allocator]
    static GLOBAL: Oxidalloc = Oxidalloc;

    #[test]
    fn test_vec_growth() {
        let mut v = Vec::new();
        for i in 0..10_000 {
            v.push(i);
        }
        assert_eq!(v.len(), 10_000);
        assert_eq!(v[0], 0);
        assert_eq!(v[9999], 9999);
    }

    #[test]
    fn test_box() {
        let b = Box::new(12345);
        assert_eq!(*b, 12345);
    }

    #[repr(align(256))]
    struct AlignedStruct(u8);

    #[test]
    fn test_aligned_allocation() {
        let b = Box::new(AlignedStruct(42));
        let ptr = &*b as *const AlignedStruct;
        assert_eq!(ptr as usize % 256, 0, "Box allocation not aligned to 256");
    }

    #[test]
    fn test_aligned_realloc() {
        let layout = Layout::from_size_align(256, 256).unwrap();
        unsafe {
            let ptr = GLOBAL.alloc(layout);
            assert!(!ptr.is_null());
            assert_eq!(ptr as usize % 256, 0);

            std::ptr::write_bytes(ptr, 0xAA, 256);

            let new_ptr = GLOBAL.realloc(ptr, layout, 512);
            assert!(!new_ptr.is_null());
            assert_eq!(
                new_ptr as usize % 256,
                0,
                "Realloc returned unaligned address"
            );

            let slice = std::slice::from_raw_parts(new_ptr, 256);
            for &byte in slice {
                assert_eq!(byte, 0xAA);
            }

            GLOBAL.dealloc(new_ptr, Layout::from_size_align(512, 256).unwrap());
        }
    }

    #[test]
    fn test_huge_allocation() {
        let mut v = vec![0u8; 10 * 1024 * 1024];
        v[0] = 1;
        let len = v.len();
        v[len - 1] = 1;
        assert_eq!(v.len(), 10 * 1024 * 1024);
    }

    #[test]
    fn test_threading() {
        let handles: Vec<_> = (0..10)
            .map(|i| {
                std::thread::spawn(move || {
                    let mut v = Vec::with_capacity(1000);
                    for j in 0..1000 {
                        v.push(i * j);
                    }
                    v.iter().sum::<usize>()
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
    }
}
