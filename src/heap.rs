use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::fmt;
use core::ptr::{self, addr_of_mut, null_mut};
use core::sync::atomic::{AtomicBool, Ordering};

const HEAP_SIZE: usize = 1024 * 1024;
const NODE_ALIGNMENT: usize = core::mem::align_of::<FreeNode>();

#[repr(C, align(4096))]
struct HeapSpace([u8; HEAP_SIZE]);

static mut HEAP_SPACE: HeapSpace = HeapSpace([0; HEAP_SIZE]);

struct FreeNode {
    size: usize,
    next: *mut FreeNode,
}

#[repr(C)]
struct AllocationHeader {
    start: *mut u8,
    size: usize,
}

struct HeapAllocator {
    initialized: AtomicBool,
    lock: AtomicBool,
    head: UnsafeCell<FreeNode>,
}

unsafe impl Sync for HeapAllocator {}

impl HeapAllocator {
    const fn new() -> Self {
        Self {
            initialized: AtomicBool::new(false),
            lock: AtomicBool::new(false),
            head: UnsafeCell::new(FreeNode {
                size: 0,
                next: null_mut(),
            }),
        }
    }

    fn acquire(&self) {
        while self
            .lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }

    fn release(&self) {
        self.lock.store(false, Ordering::Release);
    }

    unsafe fn add_free_region(&self, start: *mut u8, size: usize) {
        unsafe {
            let mut previous = self.head.get();
            let mut current = (*previous).next;
            while !current.is_null() && (current as usize) < start as usize {
                previous = current;
                current = (*current).next;
            }

            let new_node = start.cast::<FreeNode>();
            ptr::write(
                new_node,
                FreeNode {
                    size,
                    next: current,
                },
            );
            (*previous).next = new_node;

            if !current.is_null() && new_node as usize + (*new_node).size == current as usize {
                (*new_node).size += (*current).size;
                (*new_node).next = (*current).next;
            }

            if previous != self.head.get()
                && previous as usize + (*previous).size == new_node as usize
            {
                (*previous).size += (*new_node).size;
                (*previous).next = (*new_node).next;
            }
        }
    }
}

#[global_allocator]
static ALLOCATOR: HeapAllocator = HeapAllocator::new();

#[derive(Clone, Copy, Debug)]
pub enum Error {
    AlreadyInitialized,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInitialized => formatter.write_str("heap was initialized more than once"),
        }
    }
}

pub fn init() -> Result<(), Error> {
    if ALLOCATOR.initialized.swap(true, Ordering::AcqRel) {
        return Err(Error::AlreadyInitialized);
    }

    let start = unsafe { addr_of_mut!(HEAP_SPACE.0) as *mut u8 };
    ALLOCATOR.acquire();
    unsafe {
        ALLOCATOR.add_free_region(start, HEAP_SIZE);
    }
    ALLOCATOR.release();
    Ok(())
}

unsafe impl GlobalAlloc for HeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !self.initialized.load(Ordering::Acquire) {
            return null_mut();
        }

        let alignment = layout.align().max(NODE_ALIGNMENT);
        let payload_size = layout.size().max(1);
        let header_size = core::mem::size_of::<AllocationHeader>();
        self.acquire();
        let mut current = unsafe { (*self.head.get()).next };

        while !current.is_null() {
            let block_start = current as usize;
            let block_end = match block_start.checked_add(unsafe { (*current).size }) {
                Some(end) => end,
                None => break,
            };
            let minimum_user = match block_start
                .checked_add(core::mem::size_of::<FreeNode>())
                .and_then(|value| value.checked_add(header_size))
            {
                Some(value) => value,
                None => break,
            };
            let user = match align_up(minimum_user, alignment) {
                Some(value) => value,
                None => break,
            };
            let allocation_start = user - header_size;
            let payload_end = match user.checked_add(payload_size) {
                Some(value) => value,
                None => break,
            };
            let mut allocation_end = match align_up(payload_end, NODE_ALIGNMENT) {
                Some(value) => value,
                None => break,
            };
            if allocation_end > block_end {
                current = unsafe { (*current).next };
                continue;
            }

            let suffix_size = block_end - allocation_end;
            if suffix_size != 0 && suffix_size < core::mem::size_of::<FreeNode>() {
                allocation_end = block_end;
            }
            let old_next = unsafe { (*current).next };
            let prefix_size = allocation_start - block_start;
            unsafe {
                (*current).size = prefix_size;
                if allocation_end < block_end {
                    let suffix = allocation_end as *mut FreeNode;
                    ptr::write(
                        suffix,
                        FreeNode {
                            size: block_end - allocation_end,
                            next: old_next,
                        },
                    );
                    (*current).next = suffix;
                } else {
                    (*current).next = old_next;
                }

                ptr::write(
                    (user - header_size) as *mut AllocationHeader,
                    AllocationHeader {
                        start: allocation_start as *mut u8,
                        size: allocation_end - allocation_start,
                    },
                );
            }
            self.release();
            return user as *mut u8;
        }

        self.release();
        null_mut()
    }

    unsafe fn dealloc(&self, pointer: *mut u8, _layout: Layout) {
        if pointer.is_null() || !self.initialized.load(Ordering::Acquire) {
            return;
        }
        let header = unsafe {
            ptr::read(
                pointer
                    .sub(core::mem::size_of::<AllocationHeader>())
                    .cast::<AllocationHeader>(),
            )
        };
        self.acquire();
        unsafe {
            self.add_free_region(header.start, header.size);
        }
        self.release();
    }
}

fn align_up(value: usize, alignment: usize) -> Option<usize> {
    value
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
}

pub fn capacity_bytes() -> usize {
    HEAP_SIZE
}

pub fn self_test() -> bool {
    let layout = match Layout::from_size_align(257, 64) {
        Ok(layout) => layout,
        Err(_) => return false,
    };
    unsafe {
        let first = ALLOCATOR.alloc(layout);
        if first.is_null() || first as usize % 64 != 0 {
            return false;
        }
        first.write_bytes(0xa5, layout.size());
        ALLOCATOR.dealloc(first, layout);

        let second = ALLOCATOR.alloc(layout);
        if second.is_null() || second as usize % 64 != 0 {
            return false;
        }
        let valid = second == first && (0..layout.size()).all(|index| *second.add(index) == 0xa5);
        ALLOCATOR.dealloc(second, layout);
        valid
    }
}
