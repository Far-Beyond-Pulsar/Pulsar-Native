//! Memory arena for bytecode execution state.
//!
//! Provides a flat byte buffer for storing blueprint variable values during execution.
//! Variables are accessed by offset, enabling efficient zero-copy operations.

use std::alloc::{alloc, dealloc, Layout};
use std::ptr;

/// A flat byte arena for storing blueprint variable state.
///
/// Variables are laid out sequentially based on their size and alignment,
/// and accessed by byte offset. This matches the memory model used by PBGC bytecode.
pub struct ByteArena {
    /// Pointer to the allocated memory
    ptr: *mut u8,

    /// Total size of the arena in bytes
    size: usize,

    /// Memory layout (for deallocation)
    layout: Layout,
}

impl ByteArena {
    /// Create a new arena with the specified size.
    ///
    /// # Panics
    /// Panics if allocation fails or size is 0.
    pub fn new(size: usize) -> Self {
        assert!(size > 0, "Arena size must be greater than 0");

        let layout = Layout::from_size_align(size, 8).expect("Invalid arena layout");

        let ptr = unsafe { alloc(layout) };

        if ptr.is_null() {
            panic!("Failed to allocate arena memory");
        }

        // Zero-initialize the arena
        unsafe {
            ptr::write_bytes(ptr, 0, size);
        }

        Self { ptr, size, layout }
    }

    /// Get the size of the arena in bytes.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Get a raw pointer to the arena.
    ///
    /// # Safety
    /// The pointer is valid as long as this arena exists.
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr
    }

    /// Get a mutable raw pointer to the arena.
    ///
    /// # Safety
    /// The pointer is valid as long as this arena exists.
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr
    }

    /// Write a value at a specific offset.
    ///
    /// # Safety
    /// - Offset must be within bounds
    /// - Offset + size_of::<T>() must not exceed arena size
    /// - Offset must be properly aligned for T
    pub unsafe fn write_at<T>(&mut self, offset: usize, value: &T) {
        assert!(
            offset + std::mem::size_of::<T>() <= self.size,
            "Write would exceed arena bounds: offset={}, size={}, arena_size={}",
            offset,
            std::mem::size_of::<T>(),
            self.size
        );

        let dest = self.ptr.add(offset) as *mut T;
        ptr::write(dest, ptr::read(value));
    }

    /// Write raw bytes at a specific offset.
    ///
    /// # Safety
    /// - Offset must be within bounds
    /// - Offset + bytes.len() must not exceed arena size
    pub unsafe fn write_bytes_at(&mut self, offset: usize, bytes: &[u8]) {
        assert!(
            offset + bytes.len() <= self.size,
            "Write would exceed arena bounds"
        );

        let dest = self.ptr.add(offset);
        ptr::copy_nonoverlapping(bytes.as_ptr(), dest, bytes.len());
    }

    /// Read a value from a specific offset.
    ///
    /// # Safety
    /// - Offset must be within bounds
    /// - Offset + size_of::<T>() must not exceed arena size
    /// - Offset must be properly aligned for T
    /// - Value at offset must be a valid T
    pub unsafe fn read<T: Copy>(&self, offset: usize) -> T {
        assert!(
            offset + std::mem::size_of::<T>() <= self.size,
            "Read would exceed arena bounds"
        );

        let src = self.ptr.add(offset) as *const T;
        ptr::read(src)
    }

    /// Read raw bytes from a specific offset.
    ///
    /// # Safety
    /// - Offset must be within bounds
    /// - Offset + len must not exceed arena size
    pub unsafe fn read_bytes(&self, offset: usize, len: usize) -> Vec<u8> {
        assert!(offset + len <= self.size, "Read would exceed arena bounds");

        let src = self.ptr.add(offset);
        let mut bytes = vec![0u8; len];
        ptr::copy_nonoverlapping(src, bytes.as_mut_ptr(), len);
        bytes
    }

    /// Zero out the entire arena.
    pub fn clear(&mut self) {
        unsafe {
            ptr::write_bytes(self.ptr, 0, self.size);
        }
    }

    /// Get a slice view of a region of the arena.
    ///
    /// # Safety
    /// - Offset must be within bounds
    /// - Offset + len must not exceed arena size
    pub unsafe fn slice(&self, offset: usize, len: usize) -> &[u8] {
        assert!(offset + len <= self.size, "Slice would exceed arena bounds");

        std::slice::from_raw_parts(self.ptr.add(offset), len)
    }

    /// Get a mutable slice view of a region of the arena.
    ///
    /// # Safety
    /// - Offset must be within bounds
    /// - Offset + len must not exceed arena size
    pub unsafe fn slice_mut(&mut self, offset: usize, len: usize) -> &mut [u8] {
        assert!(offset + len <= self.size, "Slice would exceed arena bounds");

        std::slice::from_raw_parts_mut(self.ptr.add(offset), len)
    }
}

impl Drop for ByteArena {
    fn drop(&mut self) {
        unsafe {
            dealloc(self.ptr, self.layout);
        }
    }
}

// ByteArena is Send + Sync since it owns its memory
unsafe impl Send for ByteArena {}
unsafe impl Sync for ByteArena {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arena_creation() {
        let arena = ByteArena::new(1024);
        assert_eq!(arena.size(), 1024);
    }

    #[test]
    #[should_panic(expected = "Arena size must be greater than 0")]
    fn test_arena_zero_size() {
        ByteArena::new(0);
    }

    #[test]
    fn test_write_and_read_f32() {
        let mut arena = ByteArena::new(1024);

        unsafe {
            arena.write_at(0, &42.5_f32);
            let value: f32 = arena.read(0);
            assert_eq!(value, 42.5_f32);
        }
    }

    #[test]
    fn test_write_and_read_i32() {
        let mut arena = ByteArena::new(1024);

        unsafe {
            arena.write_at(0, &-12345_i32);
            let value: i32 = arena.read(0);
            assert_eq!(value, -12345_i32);
        }
    }

    #[test]
    fn test_write_and_read_multiple_values() {
        let mut arena = ByteArena::new(1024);

        unsafe {
            arena.write_at(0, &100.0_f32);
            arena.write_at(4, &200_i32);
            arena.write_at(8, &true);

            assert_eq!(arena.read::<f32>(0), 100.0);
            assert_eq!(arena.read::<i32>(4), 200);
            assert_eq!(arena.read::<bool>(8), true);
        }
    }

    #[test]
    fn test_write_and_read_bytes() {
        let mut arena = ByteArena::new(1024);

        let data = vec![1, 2, 3, 4, 5];

        unsafe {
            arena.write_bytes_at(0, &data);
            let read_data = arena.read_bytes(0, 5);
            assert_eq!(read_data, data);
        }
    }

    #[test]
    fn test_clear() {
        let mut arena = ByteArena::new(1024);

        unsafe {
            arena.write_at(0, &42_i32);
            arena.clear();
            assert_eq!(arena.read::<i32>(0), 0);
        }
    }

    #[test]
    #[should_panic(expected = "Write would exceed arena bounds")]
    fn test_write_out_of_bounds() {
        let mut arena = ByteArena::new(8);

        unsafe {
            arena.write_at(10, &42_i32); // Should panic
        }
    }

    #[test]
    #[should_panic(expected = "Read would exceed arena bounds")]
    fn test_read_out_of_bounds() {
        let arena = ByteArena::new(8);

        unsafe {
            let _: i32 = arena.read(10); // Should panic
        }
    }
}
