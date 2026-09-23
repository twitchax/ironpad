//! Layout arithmetic for the Enzyme C allocator shims (`enzyme_shims`).
//!
//! Split out of the shims so its overflow checks run in native tests: the
//! shims themselves are wasm32-only, because defining `malloc` on the host
//! target would replace the test binary's own allocator.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use core::alloc::Layout;

/// Header bytes prepended to every shim allocation (stores the size; keeps
/// 16-byte alignment for the payload).
pub(crate) const SHIM_HEADER: usize = 16;

/// The layout of a shim allocation holding `size` payload bytes, or `None`
/// when `size + SHIM_HEADER` overflows or exceeds what a `Layout` can
/// describe (`isize::MAX` once rounded to the alignment).
///
/// Unchecked, the sum wraps in release builds (which cells are) to a tiny
/// layout: the header write lands inside it and the returned payload pointer
/// sits past the allocation.
pub(crate) fn shim_layout(size: usize) -> Option<Layout> {
    Layout::from_size_align(size.checked_add(SHIM_HEADER)?, SHIM_HEADER).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_that_wrap_or_overflow_a_layout_are_refused() {
        assert_eq!(shim_layout(usize::MAX), None);
        assert_eq!(shim_layout(usize::MAX - 8), None);
        // Fits in usize, but past isize::MAX once the header is added.
        #[allow(clippy::cast_sign_loss)]
        let max = isize::MAX as usize;
        assert_eq!(shim_layout(max), None);
    }

    #[test]
    fn ordinary_sizes_carry_the_header() {
        for size in [0, 100] {
            let layout = shim_layout(size).expect("an ordinary size has a layout");
            assert_eq!(layout.size(), size + SHIM_HEADER);
            assert_eq!(layout.align(), SHIM_HEADER);
        }
    }
}
