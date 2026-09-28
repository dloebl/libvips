//! The C-ABI static library linked into libvips.
//!
//! Every Rust crate of libvips is linked in here, so that there is exactly
//! one copy of the Rust standard library. The crates' #[no_mangle]
//! functions are called from C, eg. vips_foreign_load_bmp_file_get_type().
//!
//! libvips/meson.build hides all symbols of this library from the shared
//! libvips, so the ABI stays defined by the C headers alone.

extern crate vips_foreign;
