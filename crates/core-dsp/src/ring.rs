//! Lock-free single-producer / single-consumer ring buffer of `f32`.
//! Safe code only: samples are stored as `AtomicU32` bit patterns.

use crate::sync::{AtomicU32, AtomicUsize};
use std::sync::atomic::Ordering::*;

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

    /// Samples queued. Safe to call from any thread, including a third one that is
    /// neither producer nor consumer (meters, self-test).
    pub fn len(&self) -> usize {
        // `tail` first: the consumer only stores a tail it has seen `head` reach, and the
        // Acquire load of that tail makes that head visible, so the `head` read next is
        // never behind it and the difference cannot wrap. (Reading `head` first let a
        // push + pop slip in between and returned usize::MAX.) The producer may refill
        // in between as well, so clamp to the capacity.
        let tail = self.tail.load(Acquire);
        let head = self.head.load(Acquire);
        head.wrapping_sub(tail).min(self.capacity())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
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

#[cfg(all(test, not(loom)))]
mod tests {
    use super::*;

    /// A third thread (UI meter, self-test) reading the fill level while audio flows
    /// must always see a value within 0..=capacity.
    #[test]
    fn len_seen_by_an_observer_is_bounded() {
        use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
        use std::sync::Arc;
        let r = Arc::new(Ring::new(64));
        let done = Arc::new(AtomicBool::new(false));
        let p = {
            let (r, done) = (r.clone(), done.clone());
            std::thread::spawn(move || {
                let x = [0.25f32; 7];
                while !done.load(Relaxed) {
                    r.push(&x);
                }
            })
        };
        let c = {
            let (r, done) = (r.clone(), done.clone());
            std::thread::spawn(move || {
                let mut b = [0.0f32; 5];
                while !done.load(Relaxed) {
                    r.pop(&mut b);
                }
            })
        };
        let mut worst = 0usize;
        for _ in 0..3_000_000 {
            worst = worst.max(r.len());
        }
        done.store(true, Relaxed);
        p.join().unwrap();
        c.join().unwrap();
        assert!(
            worst <= r.capacity(),
            "observer saw len {worst} > capacity 64"
        );
    }

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

#[cfg(all(test, loom))]
mod loom_tests {
    use super::*;
    use loom::sync::Arc;
    use loom::thread;

    /// One push and one pop race a third thread reading `len()`.
    #[test]
    fn loom_observer_len_never_exceeds_capacity() {
        loom::model(|| {
            let r = Arc::new(Ring::new(2));
            let p = {
                let r = r.clone();
                thread::spawn(move || {
                    r.push(&[1.0]);
                })
            };
            let c = {
                let r = r.clone();
                thread::spawn(move || {
                    let mut b = [0.0f32; 1];
                    if r.pop(&mut b) == 1 {
                        assert_eq!(b[0], 1.0, "a popped sample is the pushed one");
                    }
                })
            };
            let o = {
                let r = r.clone();
                thread::spawn(move || {
                    let seen = r.len();
                    assert!(seen <= r.capacity(), "observer saw len {seen}");
                })
            };
            p.join().unwrap();
            c.join().unwrap();
            o.join().unwrap();
            assert!(r.len() <= 1);
        });
    }

    /// Plain SPSC hand-off: every value pushed is popped once, in order, intact.
    #[test]
    fn loom_spsc_transfers_in_order() {
        loom::model(|| {
            let r = Arc::new(Ring::new(2));
            let p = {
                let r = r.clone();
                thread::spawn(move || {
                    let src = [0.5f32, -0.25, 0.125];
                    let mut i = 0;
                    while i < src.len() {
                        i += r.push(&src[i..]);
                        thread::yield_now();
                    }
                })
            };
            let mut out = Vec::new();
            let mut b = [0.0f32; 2];
            while out.len() < 3 {
                let n = r.pop(&mut b);
                out.extend_from_slice(&b[..n]);
                thread::yield_now();
            }
            p.join().unwrap();
            assert_eq!(out, vec![0.5, -0.25, 0.125]);
        });
    }
}
