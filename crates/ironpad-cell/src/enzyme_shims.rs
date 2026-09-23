//! C allocator shims for Enzyme (`std::autodiff`) on `wasm32-unknown-unknown`.
//!
//! Enzyme's reverse-mode pass allocates its tape through the C allocator
//! (`malloc`/`free`/`realloc`/`calloc`), which doesn't exist on
//! `wasm32-unknown-unknown`. These shims back those symbols with Rust's global
//! allocator, stashing the allocation size in a 16-byte header so `free` and
//! `realloc` (which receive no size) can reconstruct the layout.
//!
//! Signature note: Enzyme declares `realloc` with a **64-bit** size even on
//! wasm32 — a mismatched `usize` shim links, but Enzyme replaces the call with
//! a `.Lrealloc_bitcast_invalid` trap. The signatures below are exactly the
//! ones verified end to end (reverse-mode gradient through a data-dependent
//! loop, matching finite differences).
//!
//! The module is compiled unconditionally on wasm32: the symbols are inert
//! unless Enzyme-generated code calls them, and nothing else defines the C
//! allocator on this target.
#![allow(clippy::missing_safety_doc)]

use core::alloc::Layout;

use crate::shim_layout::{shim_layout, SHIM_HEADER};

#[no_mangle]
pub extern "C" fn malloc(size: usize) -> *mut u8 {
    // A size whose header-extended layout does not exist fails like any
    // oversized request, with null; `calloc` relies on that for its
    // saturated product.
    let Some(layout) = shim_layout(size) else {
        return core::ptr::null_mut();
    };
    unsafe {
        let base = std::alloc::alloc(layout);
        if base.is_null() {
            return base;
        }
        base.cast::<usize>().write(size);
        base.add(SHIM_HEADER)
    }
}

#[no_mangle]
pub extern "C" fn free(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        let base = ptr.sub(SHIM_HEADER);
        let size = base.cast::<usize>().read();
        // Unchecked is sound here: `size` was validated by `shim_layout` when
        // `malloc` allocated this block, and the header stores it unchanged.
        let layout = Layout::from_size_align_unchecked(size + SHIM_HEADER, SHIM_HEADER);
        std::alloc::dealloc(base, layout);
    }
}

#[no_mangle]
pub extern "C" fn realloc(ptr: *mut u8, new_size: u64) -> *mut u8 {
    // A size over 4 GiB cannot be served on wasm32; a truncating cast would
    // hand back a small block instead of failing (the original stays valid).
    let Ok(new_size) = usize::try_from(new_size) else {
        return core::ptr::null_mut();
    };
    if ptr.is_null() {
        return malloc(new_size);
    }
    if new_size == 0 {
        free(ptr);
        return core::ptr::null_mut();
    }
    unsafe {
        let old_size = ptr.sub(SHIM_HEADER).cast::<usize>().read();
        let new_ptr = malloc(new_size);
        if !new_ptr.is_null() {
            core::ptr::copy_nonoverlapping(ptr, new_ptr, old_size.min(new_size));
            free(ptr);
        }
        new_ptr
    }
}

#[no_mangle]
pub extern "C" fn calloc(count: usize, size: usize) -> *mut u8 {
    let total = count.saturating_mul(size);
    let ptr = malloc(total);
    if !ptr.is_null() {
        unsafe { core::ptr::write_bytes(ptr, 0, total) };
    }
    ptr
}
