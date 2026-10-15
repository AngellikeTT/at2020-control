//! Lock-free single-producer / single-consumer ring buffer of `f32`.
//! Safe code only: samples are stored as `AtomicU32` bit patterns.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering::*};

pub struct Ring {
    buf: Box<[AtomicU32]>,
    mask: usize,
    head: AtomicUsize, // total samples written (producer-owned)
    tail: AtomicUsize, // total samples read (consumer-owned)
}

impl Ring {
    pub fn new(capacity_pow2: usize) -> Self {
        assert!(capacity_pow2.is_power_of_two());
        let buf: Vec<AtomicU32> = (0..capacity_pow2).map(|_| AtomicU32::new(0)).collect();
        Self {
            buf: buf.into_boxed_slice(),
            mask: capacity_pow2 - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    pub fn capacity(&self) -> usize {
        self.mask + 1
    }

    pub fn len(&self) -> usize {
        self.head
            .load(Acquire)
            .wrapping_sub(self.tail.load(Acquire))
    }

    /// Producer side. Returns how many samples were accepted.
    pub fn push(&self, src: &[f32]) -> usize {
        let head = self.head.load(Relaxed);
        let tail = self.tail.load(Acquire);
        let free = self.capacity() - head.wrapping_sub(tail);
        let n = src.len().min(free);
        for (i, s) in src[..n].iter().enumerate() {
            self.buf[head.wrapping_add(i) & self.mask].store(s.to_bits(), Relaxed);
        }
        self.head.store(head.wrapping_add(n), Release);
        n
    }

    /// Consumer side. Returns how many samples were read.
    pub fn pop(&self, dst: &mut [f32]) -> usize {
        let tail = self.tail.load(Relaxed);
        let head = self.head.load(Acquire);
        let n = dst.len().min(head.wrapping_sub(tail));
        for (i, d) in dst[..n].iter_mut().enumerate() {
            *d = f32::from_bits(self.buf[tail.wrapping_add(i) & self.mask].load(Relaxed));
        }
        self.tail.store(tail.wrapping_add(n), Release);
        n
    }

    /// Consumer side: drop everything currently queued.
    #[cfg(test)]
    pub fn discard_all(&self) {
        self.tail.store(self.head.load(Acquire), Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_and_preserves_order() {
        let r = Ring::new(8);
        let mut out = [0.0f32; 8];
        let mut next = 0.0f32;
        let mut expect = 0.0f32;
        for _ in 0..100 {
            let data: Vec<f32> = (0..5)
                .map(|_| {
                    next += 1.0;
                    next
                })
                .collect();
            assert_eq!(r.push(&data), 5);
            let n = r.pop(&mut out[..5]);
            assert_eq!(n, 5);
            for v in &out[..5] {
                expect += 1.0;
                assert_eq!(*v, expect);
            }
        }
    }

    #[test]
    fn push_is_bounded() {
        let r = Ring::new(4);
        assert_eq!(r.push(&[1.0; 10]), 4);
        assert_eq!(r.len(), 4);
        r.discard_all();
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn threads() {
        use std::sync::Arc;
        let r = Arc::new(Ring::new(1024));
        let p = r.clone();
        let t = std::thread::spawn(move || {
            let mut i = 0u32;
            while i < 200_000 {
                if p.push(&[i as f32]) == 1 {
                    i += 1;
                }
            }
        });
        let mut expect = 0u32;
        let mut b = [0.0f32; 1];
        while expect < 200_000 {
            if r.pop(&mut b) == 1 {
                assert_eq!(b[0], expect as f32);
                expect += 1;
            }
        }
        t.join().unwrap();
    }
}
