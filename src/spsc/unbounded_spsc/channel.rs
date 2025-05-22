use super::raw_spsc::RawSpsc;
use std::sync::Arc;

/// Prevents Clone and Copy at compile time.
#[derive(Debug)]
struct NoClone;


/// A factory type for creating a single-producer single-consumer (SPSC) channel.
///
/// This channel is unbounded and non-blocking. It provides one [`Sender`] and one [`Receiver`]
/// which are safe to move across threads, but **must not be cloned**.
/// Internally backed by a lock-free queue [`RawSpsc`].
pub struct UnboundSpscChannel;

/// The sending half of an [`UnboundSpscChannel`].
///
/// This struct wraps an atomic reference to the underlying queue.  
/// Only a single instance should exist—cloning or sharing between multiple producers is **undefined behavior**.
#[repr(transparent)]
pub struct Sender<T> {
    inner: Arc<RawSpsc<T>>,
    _no_clone: NoClone,
}

/// The receiving half of an [`UnboundSpscChannel`].
///
/// Like [`Sender`], this should not be cloned. Only one thread should consume from the channel.
#[repr(transparent)]
pub struct Receiver<T> {
    inner: Arc<RawSpsc<T>>,
    _no_clone: NoClone,
}

impl<T> Sender<T> {
    /// Sends a value into the channel.
    ///
    /// # Panics
    /// This function does not panic under normal usage.
    ///
    /// # Safety
    /// The channel must follow the SPSC model—only one sender thread must exist.
    #[inline]
    pub fn send(&self, value: T) {
        self.inner.push(value);
    }
}

impl<T> Receiver<T> {
    /// Receives a value from the channel, or returns [`None`] if the channel is empty.
    ///
    /// # Safety
    /// Only one receiver thread must call this method.
    #[inline]
    pub fn recv(&self) -> Option<T> {
        self.inner.pop()
    }
}

impl UnboundSpscChannel {
    /// Creates a new unbounded SPSC channel.
    ///
    /// Returns a tuple of `(Sender<T>, Receiver<T>)`, which represent the only producer
    /// and consumer endpoints respectively.
    ///
    /// Internally, the shared `RawSpsc<T>` queue is wrapped in an [`Arc`] and passed to both ends.
    ///
    /// # Panics
    /// Panics if the underlying queue allocation fails.
    pub fn split<T>() -> (Sender<T>, Receiver<T>) {
        let inner = Arc::new(RawSpsc::new());
        (Sender { inner: inner.clone(), _no_clone: NoClone }, Receiver { inner,_no_clone: NoClone })
    }
}

unsafe impl<T> Send for Sender<T> {}
unsafe impl<T> Sync for Sender<T> {}
unsafe impl<T> Send for Receiver<T> {}
unsafe impl<T> Sync for Receiver<T> {}


#[cfg(test)]
mod tests {
    use std::thread;
    use super::UnboundSpscChannel;

    const COUNT: usize = 100_000;

    #[test]
    fn single_thread_send_recv() {
        let (sender, receiver) = UnboundSpscChannel::split();

        for i in 0..1000 {
            sender.send(i);
            let val = receiver.recv();
            assert_eq!(val, Some(i));
        }

        assert_eq!(receiver.recv(), None); // queue is now empty
    }

    #[test]
    fn batch_send_recv() {
        let (sender, receiver) = UnboundSpscChannel::split();

        for i in 0..COUNT {
            sender.send(i);
        }

        for i in 0..COUNT {
            let val = receiver.recv();
            assert_eq!(val, Some(i));
        }

        assert_eq!(receiver.recv(), None); // Should now be empty
    }

    #[test]
    fn spsc_threaded_test() {
        let (sender, receiver) = UnboundSpscChannel::split();

        let producer = thread::spawn(move || {
            for i in 0..COUNT {
                sender.send(i);
            }
        });

        let consumer = thread::spawn(move || {
            for i in 0..COUNT {
                loop {
                    if let Some(val) = receiver.recv() {
                        assert_eq!(val, i);
                        break;
                    }
                }
            }

            assert_eq!(receiver.recv(), None); // empty check
        });

        producer.join().unwrap();
        consumer.join().unwrap();
    }
}
