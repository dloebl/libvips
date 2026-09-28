//! Loaders and savers implemented in Rust, see libvips/rust/README.md.
//!
//! Each module registers its types from a `*_get_type()` function, called
//! from vips_foreign_operation_init() in foreign.c.

mod bmpload;
