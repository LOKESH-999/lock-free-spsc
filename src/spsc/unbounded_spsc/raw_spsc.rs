use crate::cache_padded::CachePadded;
use std::alloc::{Layout, alloc, dealloc};

use std::ptr::null_mut;
use std::sync::atomic::AtomicBool;
use std::{
    sync::atomic::{
        AtomicPtr,
        Ordering::{Acquire, Relaxed, Release},
    },
};
use super::super::bounded_spsc::inner_spsc::BoundedSpsc;

pub struct RawSpsc<T> {
    pub(super) curr: CachePadded<AtomicPtr<BoundedSpsc<T>>>,
    pub(super) next: CachePadded<AtomicPtr<BoundedSpsc<T>>>,
}

pub struct InnerSpsc<T> {
    pub(super) head: CachePadded<AtomicPtr<RawSpsc<T>>>,
    pub(super) tail: CachePadded<AtomicPtr<RawSpsc<T>>>,
    pub(super) is_bulding: AtomicBool,
}

impl<T> RawSpsc<T> {
    pub fn new(capacity: usize) -> Self {
        let layout = Layout::array::<BoundedSpsc<T>>(capacity).unwrap();
        let ptr = unsafe { alloc(layout) as *mut BoundedSpsc<T> };
        let next = CachePadded::new(AtomicPtr::new(ptr));
        let curr = CachePadded::new(AtomicPtr::new(null_mut()));
        RawSpsc { curr, next }
    }

    pub fn push(&self, value: T) -> Result<(), T> {
        let curr = self.curr.load(Relaxed);
        unsafe {
            (*curr).push(value)
        }
    }

    pub fn pop(&self) -> Option<T> {
        let curr = self.curr.load(Relaxed);
        unsafe {
            (*curr).pop()
        }
    }
    

}



impl<T> InnerSpsc<T> {
    pub fn new(capacity: usize) -> Self {
        let layout = Layout::array::<RawSpsc<T>>(capacity).unwrap();
        let ptr = unsafe { alloc(layout) as *mut RawSpsc<T> };
        let head = CachePadded::new(AtomicPtr::new(ptr));
        let tail = CachePadded::new(AtomicPtr::new(null_mut()));
        let is_bulding = AtomicBool::new(false);
        InnerSpsc { head, tail, is_bulding }
    }
}
