//! Lock-free, bounded, single-producer single-consumer (SPSC) channel.
//!
//! This module provides a bounded SPSC channel built on top of a manually
//! managed ring buffer. It guarantees zero allocations after initialization,
//! is cache-friendly, and is safe for concurrent use across one sender and one receiver thread.
//!
//! # Overview
//! The [`BoundedSpscChannel`] type provides a way to construct a bounded channel,
//! returning a [`Sender`] and a [`Receiver`] pair. The underlying buffer is backed
//! by [`BoundedSpsc`] from [`inner_spsc`](super::inner_spsc), which manages wraparound
//! logic and data placement with manual memory control.
//!
//! This channel is ideal for real-time or high-throughput systems
//! where performance and memory determinism are critical.
//!
//! # Example
//! ```
//! use mycrate::spsc::bounded_spsc::channel::BoundedSpscChannel;
//!
//! let (tx, rx) = BoundedSpscChannel::split(32);
//!
//! tx.send(42).unwrap();
//! assert_eq!(rx.recv(), Some(42));
//! ```
//!
//! # Internals
//! Internally, the implementation wraps a [`BoundedSpsc<T>`] in an `Arc`
//! so that the producer (`Sender<T>`) and consumer (`Receiver<T>`) can safely
//! share the same buffer. Operations are wait-free under ideal conditions and
//! make use of memory ordering for correct synchronization between threads.
//!
//! See [`inner_spsc`](super::inner_spsc) for detailed explanation of how
//! wraparound, index updates, and buffer safety are handled.

use super::inner_spsc::BoundedSpsc;
use std::sync::Arc;

/// Entry point for splitting a bounded SPSC channel into its sender and receiver halves.
pub struct BoundedSpscChannel;

impl BoundedSpscChannel {
    /// Creates a bounded channel with the specified capacity.
    ///
    /// Returns a pair of [`Sender`] and [`Receiver`] handles that share
    /// the same underlying buffer. Capacity must be greater than 0.
    pub fn split<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
        let inner = BoundedSpsc::new(capacity);
        let sender = Sender {
            inner: Arc::new(inner),
        };
        let receiver = Receiver {
            inner: Arc::clone(&sender.inner),
        };
        (sender, receiver)
    }
}

/// The sending half of a bounded SPSC channel.
///
/// This type is cloneable and allows sending values into the queue.
/// It fails with the original value if the buffer is full.
pub struct Sender<T> {
    inner: Arc<BoundedSpsc<T>>,
}

impl<T> Sender<T> {
    /// Attempts to send a value into the channel.
    ///
    /// Returns `Err(value)` if the buffer is full.
    #[inline(always)]
    pub fn send(&self, value: T) -> Result<(), T> {
        self.inner.push(value)
    }

    /// Returns `true` if the channel is currently full.
    #[inline(always)]
    pub fn is_full(&self) -> bool {
        self.inner.is_full()
    }

    /// Returns the capacity of the channel.
    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    /// Returns `true` if the channel is empty.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// The receiving half of a bounded SPSC channel.
///
/// This type is cloneable and allows receiving values from the queue.
/// It returns `None` when the buffer is empty.
pub struct Receiver<T> {
    inner: Arc<BoundedSpsc<T>>,
}

impl<T> Receiver<T> {
    /// Attempts to receive a value from the channel.
    ///
    /// Returns `None` if the buffer is empty.
    #[inline(always)]
    pub fn recv(&self) -> Option<T> {
        self.inner.pop()
    }

    /// Returns `true` if the channel is full.
    #[inline(always)]
    pub fn is_full(&self) -> bool {
        self.inner.is_full()
    }

    /// Returns the capacity of the channel.
    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    /// Returns `true` if the channel is empty.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}
