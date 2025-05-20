use crate::cache_padded::CachePadded;
use std::alloc::{Layout, alloc, dealloc};
use std::{
    mem::MaybeUninit,
    ptr::NonNull,
    sync::atomic::{
        AtomicUsize,
        Ordering::{Acquire, Relaxed, Release},
    },
};

/// This struct will be wrapped by the [`super::channel::BoundedSpscChannel`] module as a
/// bounded single-producer single-consumer queue implementation.
///
/// [`BoundedSpscChannel`]: super::channel::BoundedSpscChannel
///
/// # Overview
///
/// `BoundedSpsc` provides a lock-free, bounded capacity queue designed for exactly
/// one producer and one consumer thread.
///
/// It uses a ring buffer internally and atomic indices to manage concurrency.
///
/// # Cache padding
///
/// The atomic indices are wrapped in [`CachePadded`] to avoid false sharing
/// across CPU cache lines. [`CachePadded`] is copied from the [`crossbeam`](https://crates.io/crates/crossbeam) crate.
///
/// # Wrap-around logic
///
/// The indices `head` and `tail` cycle through `0..capacity` with wrap-around.
/// This wrap-around is implemented using modular arithmetic, but instead of
/// the classical `% capacity`, a multiplication by a boolean mask `(index < capacity)`
/// is used to achieve wrap-around for high performance.:
///
/// ```rust
/// // if index + 1 == capacity, wrap to zero; else index + 1
/// let index = 3;
/// let capacity = 4;
/// let next_index = (index + 1) * ((index + 1) < capacity) as usize;
/// ```
///
/// This modular-like wrap-around logic ensures that the indices stay within
/// the bounds `0..capacity - 1` efficiently without the performance cost
/// of modulo operation.
///
/// # Note on capacity
///
/// The capacity must be greater than 1. One slot is always left empty to distinguish
/// full and empty states.
///
/// # Thread safety
///
/// Supports exactly one producer and one consumer thread concurrently.
///
/// # Example
///
/// See the [`super::channel::BoundedSpscChannel`] module for usage examples.
pub(crate) struct BoundedSpsc<T> {
    next_head: CachePadded<AtomicUsize>,
    tail: CachePadded<AtomicUsize>,
    buffer: Array<T>,
}

struct Array<T> {
    buffer: NonNull<MaybeUninit<T>>,
    capacity: usize,
}

impl<T> Array<T> {
    fn new(capacity: usize) -> Self {
        let layout = Layout::array::<MaybeUninit<T>>(capacity).expect("Invalid layout");
        let ptr = unsafe { alloc(layout) as *mut MaybeUninit<T> };
        let buffer = NonNull::new(ptr).expect("Failed to allocate memory");
        Self { buffer, capacity }
    }

    /// Inserts a value at `index` in the buffer.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the slot at `index` is safe to overwrite.
    #[inline(always)]
    pub(crate) unsafe fn insert(&self, index: usize, value: T) {
        unsafe {
            let ptr = self.buffer.as_ptr().add(index);
            ptr.write(MaybeUninit::new(value));
        }
    }

    /// Returns the value at `index` from the buffer.
    ///
    /// # Safety
    ///
    /// The caller must guarantee the slot at `index` is initialized.
    #[inline(always)]
    pub(crate) unsafe fn get(&self, index: usize) -> T {
        unsafe {
            let ptr = self.buffer.as_ptr().add(index);
            (*ptr).assume_init_read()
        }
    }
}

impl<T> Drop for Array<T> {
    fn drop(&mut self) {
        unsafe {
            let layout = Layout::array::<MaybeUninit<T>>(self.capacity).unwrap();
            dealloc(self.buffer.as_ptr() as *mut u8, layout);
        }
    }
}

impl<T> BoundedSpsc<T> {
    /// Creates a new `BoundedSpsc` queue with the specified capacity.
    ///
    /// # Wrap-around logic
    ///
    /// The capacity defines the size of the ring buffer.
    /// Indices will wrap around from `capacity - 1` back to `0`.
    ///
    /// The wrap-around is implemented in methods by multiplying the
    /// incremented index by a mask:
    ///
    /// ```rust
    /// let index = 3;
    /// let capacity = 4;
    /// let next_index = (index + 1) * ((index + 1) < capacity) as usize;
    /// ```
    pub(crate) fn new(capacity: usize) -> Self {
        let buffer = Array::new(capacity);
        let next_head = CachePadded::new(AtomicUsize::new(0));
        let tail = CachePadded::new(AtomicUsize::new(0));
        Self {
            next_head,
            tail,
            buffer,
        }
    }

    /// Attempts to push a value into the queue.
    ///
    /// Returns `Ok(())` if successful or `Err(value)` if the queue is full.
    ///
    /// # Wrap-around logic
    ///
    /// Calculates `next_head` by wrapping the incremented head index:
    ///
    /// ```text
    ///
    /// let curr_head = self.next_head.load(Relaxed);
    /// let next_head = (curr_head + 1) * ((curr_head + 1) < self.buffer.capacity) as usize;
    /// ```
    ///
    /// This ensures that `next_head` wraps back to zero once it reaches the capacity.
    #[inline(always)]
    pub(crate) fn push(&self, value: T) -> Result<(), T> {
        let curr_head = self.next_head.load(Relaxed);

        // Wraparound logic:
        // if curr_head + 1 == capacity, wrap to zero; else curr_head + 1
        // or equivalently: (curr_head + 1) % capacity
        let is_less = ((curr_head + 1) < self.buffer.capacity) as usize;
        let next_head = (curr_head + 1) * is_less;

        if next_head == self.tail.load(Acquire) {
            return Err(value); // Queue is full
        }

        unsafe { self.buffer.insert(curr_head, value) };
        self.next_head.store(next_head, Release);
        Ok(())
    }

    /// Attempts to pop a value from the queue.
    ///
    /// Returns `Some(value)` if available or `None` if the queue is empty.
    ///
    /// # Wrap-around logic
    ///
    /// Calculates `next_tail` by wrapping the incremented tail index:
    ///
    /// ```text
    /// let curr_tail = self.tail.load(Relaxed);
    /// let next_tail = (curr_tail + 1) * ((curr_tail + 1) < self.buffer.capacity) as usize;
    /// ```
    ///
    /// This logic wraps the tail index to zero once it reaches capacity.
    #[inline(always)]
    pub(crate) fn pop(&self) -> Option<T> {
        let curr_tail = self.tail.load(Relaxed);

        if self.next_head.load(Acquire) == curr_tail {
            return None; // Queue is empty
        }

        let value = unsafe { self.buffer.get(curr_tail) };

        // Wraparound logic:
        // if curr_tail + 1 == capacity, wrap to zero; else curr_tail + 1
        // or equivalently: (curr_tail + 1) % capacity
        let is_less = ((curr_tail + 1) < self.buffer.capacity) as usize;
        let next_tail = (curr_tail + 1) * is_less;

        self.tail.store(next_tail, Release);
        Some(value)
    }

    /// Returns `true` if the queue is empty.
    #[inline(always)]
    pub(crate) fn is_empty(&self) -> bool {
        self.next_head.load(Acquire) == self.tail.load(Acquire)
    }

    /// Returns `true` if the queue is full.
    ///
    /// # Wrap-around logic
    ///
    /// Uses the same wrap-around calculation for `next_head` as in `push`.
    #[inline(always)]
    pub(crate) fn is_full(&self) -> bool {
        let curr_head = self.next_head.load(Acquire);
        let next_head = (curr_head + 1) * ((curr_head + 1) < self.buffer.capacity) as usize;
        next_head == self.tail.load(Acquire)
    }

    /// Returns the capacity of the queue.
    #[inline(always)]
    pub(crate) fn capacity(&self) -> usize {
        self.buffer.capacity
    }
}

impl<T> Drop for BoundedSpsc<T> {
    fn drop(&mut self) {
        unsafe {
            let head = self.next_head.load(Acquire);
            let tail = self.tail.load(Acquire);
            let mut idx = tail;
            // Iterate from tail to head, dropping all initialized elements
            while idx != head {
                let ptr = self.buffer.buffer.as_ptr().add(idx);
                (*ptr).assume_init_drop();
                idx += 1;
                // Wrap-around logic on drop iterator as well
                idx *= (idx < self.buffer.capacity) as usize;
            }
        }
    }
}
