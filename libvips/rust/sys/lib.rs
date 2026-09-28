//! Raw bindings to libvips, and the parts of GLib and GObject it exposes.
//!
//! The declarations are generated from the libvips headers with bindgen at
//! build time (see ../meson.build), so struct layouts come from libclang
//! exactly as the C compiler sees them, and there is nothing to keep in sync
//! by hand. bindgen also emits compile-time checks of every struct size and
//! field offset.
//!
//! Everything here is unsafe. Operations should use the `vips` crate, and
//! only reach for these where it has no wrapper yet.

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unknown_lints,
    unnecessary_transmutes,
    clippy::all,
    clippy::undocumented_unsafe_blocks
)]
mod bindings;

pub use bindings::*;

// Macros bindgen can't evaluate.

pub const VIPS_ARGUMENT_REQUIRED_INPUT: VipsArgumentFlags =
    VIPS_ARGUMENT_INPUT | VIPS_ARGUMENT_REQUIRED | VIPS_ARGUMENT_CONSTRUCT;
pub const VIPS_ARGUMENT_OPTIONAL_INPUT: VipsArgumentFlags =
    VIPS_ARGUMENT_INPUT | VIPS_ARGUMENT_CONSTRUCT;
pub const VIPS_ARGUMENT_REQUIRED_OUTPUT: VipsArgumentFlags =
    VIPS_ARGUMENT_OUTPUT | VIPS_ARGUMENT_REQUIRED | VIPS_ARGUMENT_CONSTRUCT;
pub const VIPS_ARGUMENT_OPTIONAL_OUTPUT: VipsArgumentFlags =
    VIPS_ARGUMENT_OUTPUT | VIPS_ARGUMENT_CONSTRUCT;

/// `G_PARAM_READWRITE`, which every vips argument uses.
pub const G_PARAM_READWRITE: GParamFlags = G_PARAM_READABLE | G_PARAM_WRITABLE;
