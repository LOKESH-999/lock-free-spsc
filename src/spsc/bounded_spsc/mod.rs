mod channel;
pub(crate) mod inner_spsc;


pub use channel::BoundedSpscChannel;

#[cfg(test)]
mod tests {
    use super::BoundedSpscChannel;
    use std::thread;

    #[test]
    fn basic_push_pop() {
        let (sender, receiver) = BoundedSpscChannel::split(1);
        assert!(sender.send(10).is_ok());
        assert_eq!(receiver.recv(), Some(10));
    }

    #[test]
    fn full_and_empty() {
        let (sender, receiver) = BoundedSpscChannel::split(2);
        assert!(sender.send(42).is_ok());
        assert!(sender.send(42).is_ok());
        assert!(sender.send(99).is_err()); // Should be full
        assert_eq!(receiver.recv(), Some(42));
        assert_eq!(receiver.recv(), Some(42));
        assert_eq!(receiver.recv(), None); // Now empty
    }

    #[test]
    fn multithreaded_push_pop() {
        let (sender, receiver) = BoundedSpscChannel::split(100_000);
        let t = thread::spawn(move || {
            for i in 0..100_000u64 {
                while sender.send(i).is_err() {} // retry if full
            }
        });

        let mut sum = 0;
        for _ in 0..100_000 {
            loop {
                if let Some(v) = receiver.recv() {
                    sum += v;
                    break;
                }
            }
        }
        t.join().unwrap();
        assert_eq!(sum, (0..100_000u64).sum());
    }
}
