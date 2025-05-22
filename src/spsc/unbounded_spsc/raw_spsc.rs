use crate::cache_padded::CachePadded;
use std::alloc::{Layout, alloc, dealloc};
use std::mem::MaybeUninit;
use std::ptr::{NonNull, null_mut};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{
    AtomicPtr,
    Ordering::{Acquire, Relaxed, Release},
};

const SEGMENT_SIZE: usize = 128;
const MASK: usize = 0x7F;

/// A lock-free single-producer single-consumer (SPSC) queue implemented as a linked list of fixed-size segments.
///
/// This queue internally maintains a linked list of [`Segment`]s, each containing a fixed-size ring buffer of `T` elements.
/// The queue operates in FIFO order:
///
/// - When there is only one segment and the consumer pops elements before the segment's internal buffer fills up,
///   the queue *reuses* the same segment indefinitely without allocating new segments.
///   This means that as long as the consumer keeps pace and removes items before the buffer becomes full,
///   the producer and consumer will operate efficiently within the same pre-allocated memory block.
///
/// - If the producer outpaces the consumer and the current segment's buffer fills up (i.e., the segment is full),
///   a new segment is allocated and linked to the queue to accommodate further pushed items.
///
/// Thus, the queue behaves like a FIFO queue backed by a segmented ring buffer that grows only when necessary.
///
/// # Memory Management
/// Each segment allocates a contiguous block of uninitialized memory for `SEGMENT_SIZE` elements of type `T`.
/// Elements are pushed and popped using atomic operations on head and tail indices within the segment.
/// When a segment becomes empty and is no longer needed, it is dropped and its memory deallocated.
///
/// # Safety
/// Unsafe code is used internally to manage manual allocation and deallocation of memory and to read/write uninitialized memory.
/// The queue assumes single-producer and single-consumer threads only.
///
/// # Usage
/// This low-level `RawSpsc` queue is intended to be wrapped by the higher-level [`UnboundSpsc`] abstraction,
/// which provides a more user-friendly interface and additional functionality.
pub struct RawSpsc<T> {
    head: CachePadded<AtomicPtr<Segment<T>>>,
    tail: CachePadded<AtomicPtr<Segment<T>>>,
}

impl<T> RawSpsc<T> {
    /// Creates a new `RawSpsc` queue with a single allocated segment.
    pub fn new() -> Self {
        let segment = Box::new(Segment::new());
        let segment_ptr = Box::into_raw(segment);
        let head = CachePadded::new(AtomicPtr::new(segment_ptr));
        let tail = CachePadded::new(AtomicPtr::new(segment_ptr));

        RawSpsc { head, tail }
    }

    /// Attempts to push a value into the queue.
    ///
    /// If the current tail segment is full, a new segment is allocated and linked.
    pub fn push(&self, value: T) {
        let tail = self.tail.load(Acquire);
        let segment = unsafe { &*tail };
        match unsafe { segment.push(value) } {
            Ok(()) => return,
            Err(val) => {
                let new_block_ptr = unsafe { segment.link_and_push(val) };
                self.tail.store(new_block_ptr, Release);
            }
        }
    }

    /// Attempts to pop a value from the queue.
    ///
    /// If the current head segment is empty but there is a next linked segment,
    /// it advances to the next segment and pops from there.
    pub fn pop(&self) -> Option<T> {
        let head = self.head.load(Acquire);
        let curr_head = unsafe { &*head };
        let res = unsafe { curr_head.pop() };

        match res {
            Some(val) => Some(val),
            None => {
                let curr_tail = self.tail.load(Acquire);
                if head == curr_tail {
                    return None; // Queue is empty
                }
                // Move to next segment
                let curr_segment = unsafe { Box::from_raw(head) };
                let next_segment_ptr = curr_segment.next_block.load(Acquire);
                self.head.store(next_segment_ptr, Release);
                let next_segment = unsafe { &*next_segment_ptr };
                unsafe { next_segment.pop() }
            }
        }
    }
}

impl<T> Drop for RawSpsc<T> {
    /// Drops all linked segments starting from the head up to the tail,
    /// ensuring no memory leaks occur.
    fn drop(&mut self) {
        let head = self.head.load(Acquire);
        let tail = self.tail.load(Acquire);
        let mut curr = head;

        while curr != tail {
            let segment = unsafe { Box::from_raw(curr) };
            curr = segment.next_block.load(Acquire);
        }
        // Drop the tail segment
        _ = unsafe { Box::from_raw(curr) };
    }
}

/// Internal queue segment containing a fixed-size ring buffer of elements.
///
/// Each `Segment` maintains atomic head and tail indices to manage push/pop operations in a lock-free manner.
/// When full, new segments are allocated and linked dynamically.
///
/// # Details
/// - `next_head` is the atomic index where the producer will push the next element.
/// - `tail` is the atomic index where the consumer will pop the next element.
/// - `ptr` points to a contiguous buffer of `MaybeUninit<T>` elements of size `SEGMENT_SIZE`.
/// - `next_block` is an atomic pointer to the next `Segment` in the linked list.
///
/// The ring buffer uses wrapping arithmetic modulo `SEGMENT_SIZE`.
struct Segment<T> {
    next_head: CachePadded<AtomicUsize>,
    tail: CachePadded<AtomicUsize>,
    ptr: NonNull<MaybeUninit<T>>,
    next_block: AtomicPtr<Segment<T>>,
}

impl<T> Segment<T> {
    /// Creates a new `Segment` with an allocated buffer of `SEGMENT_SIZE` elements.
    pub fn new() -> Self {
        let next_head = CachePadded::new(AtomicUsize::new(0));
        let tail = CachePadded::new(AtomicUsize::new(0));

        let layout = Layout::array::<MaybeUninit<T>>(SEGMENT_SIZE).unwrap();
        let ptr = NonNull::new(unsafe { alloc(layout) as *mut MaybeUninit<T> })
            .expect("unable to allocate Memory");

        let next_block = null_mut::<Segment<T>>().into();

        Segment {
            next_head,
            tail,
            ptr,
            next_block,
        }
    }

    /// Attempts to push a value into this segment.
    ///
    /// Returns `Ok(())` on success, or `Err(value)` if the segment is full.
    ///
    /// # Safety
    /// This function is unsafe because it performs raw pointer writes.
    pub unsafe fn push(&self, value: T) -> Result<(), T> {
        let curr_head = self.next_head.load(Relaxed);
        let next_head = (curr_head + 1) & MASK;

        if next_head == self.tail.load(Acquire) {
            return Err(value); // Segment is full
        }
        unsafe  {
            let ptr = self.ptr.as_ptr().add(curr_head);
            (*ptr).write(value);
        }
        self.next_head.store(next_head, Release);
        Ok(())
    }

    /// Attempts to pop a value from this segment.
    ///
    /// Returns `Some(value)` if successful, or `None` if the segment is empty.
    ///
    /// # Safety
    /// This function is unsafe because it performs raw pointer reads.
    pub unsafe fn pop(&self) -> Option<T> {
        let curr_tail = self.tail.load(Relaxed);
        if self.next_head.load(Acquire) == curr_tail {
            return None; // Segment is empty
        }

        let value = unsafe {(*self.ptr.as_ptr().add(curr_tail)).assume_init_read()};
        let next_tail = (curr_tail + 1) & MASK;

        self.tail.store(next_tail, Release);
        Some(value)
    }

    /// Allocates a new segment and links it as the next block of this segment.
    ///
    /// Returns a raw pointer to the newly created segment.
    pub fn link_new_block(&self) -> *mut Segment<T> {
        let new_block_segment = Box::new(Self::new());
        let next_block = Box::into_raw(new_block_segment);
        self.next_block.store(next_block, Release);
        next_block
    }

    /// Allocates a new segment, links it as the next block, and pushes the value into it.
    ///
    /// Returns a raw pointer to the newly created segment.
    ///
    /// # Safety
    /// This function is unsafe because it dereferences raw pointers.
    pub unsafe fn link_and_push(&self, value: T) -> *mut Segment<T> {
        let new_block_ptr = self.link_new_block();
        unsafe {
            let new_block = &*new_block_ptr;
            _ = new_block.push(value);
        }
        new_block_ptr
    }
}

impl<T> Drop for Segment<T> {
    /// Drops all initialized elements within the segment and deallocates its buffer.
    fn drop(&mut self) {
        let head = self.next_head.load(Acquire);
        let tail = self.tail.load(Acquire);
        let mut idx = tail;
        // Iterate from tail to head, dropping all initialized elements
        while idx != head {
            unsafe {
                let ptr = self.ptr.as_ptr().add(idx);
                (*ptr).assume_init_drop();
            }
            idx += 1;
            idx &= MASK;
        }

        let layout = Layout::array::<MaybeUninit<T>>(SEGMENT_SIZE).unwrap();
        let ptr = self.ptr.as_ptr() as *mut u8;

        unsafe {
            dealloc(ptr, layout);
        }
    }
}


#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use super::RawSpsc; // adjust path if needed

    const COUNT: usize = 100_000;

    #[test]
    fn basic_push_pop_test() {
        let queue = RawSpsc::new();
        for i in 0..1000 {
            queue.push(i);
            let popped = queue.pop();
            assert_eq!(popped, Some(i));
        }
    }

    #[test]
    fn batch_push_pop_test() {
        let queue = RawSpsc::new();

        for i in 0..COUNT {
            queue.push(i);
        }

        for i in 0..COUNT {
            let val = queue.pop();
            assert_eq!(val, Some(i));
        }

        assert_eq!(queue.pop(), None); // Should now be empty
    }

    #[test]
    fn spsc_contention_test() {
        let queue = Arc::new(RawSpsc::new());
        let producer = {
            let queue = queue.clone();
            thread::spawn(move || {
                for i in 0..COUNT {
                    queue.push(i);
                }
            })
        };

        let consumer = {
            let queue = queue.clone();
            thread::spawn(move || {
                for i in 0..COUNT {
                    loop {
                        if let Some(val) = queue.pop() {
                            assert_eq!(val, i);
                            break;
                        }
                        // spin wait (no sleep to keep performance)
                    }
                }
            })
        };

        producer.join().unwrap();
        consumer.join().unwrap();
    }
}
