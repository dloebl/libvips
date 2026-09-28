//! Building blocks for implementing libvips operations in Rust.
//!
//! Operations are GObject subclasses, just like in C, so libvips, the C API
//! and language bindings can't tell them apart. This crate keeps the unsafe
//! parts in one place:
//!
//! - [`object`] registers types, installs arguments (the `VIPS_ARG_*`
//!   macros) and the `build` vfunc
//! - [`foreign`] does the same for loaders
//! - [`image`] and [`region`] give typed, bounds-checked pixel access
//! - [`generate`] wraps the start / generate / stop callbacks of
//!   `vips_image_generate()`
//! - [`error`] has the guard every callback from C goes through, which
//!   turns errors and panics into a vips error and a `-1` return
//!
//! See `../README.md` for how the pieces fit together.

pub use vips_sys as sys;

pub mod error;
pub mod foreign;
pub mod generate;
pub mod image;
pub mod object;
pub mod region;

pub use error::{Error, Result};
pub use generate::Generate;
pub use image::{Image, Pixel};
pub use object::{Arg, Class, Field, ObjectSubclass};
pub use region::{OutRegion, Rect, Region};

#[cfg(test)]
mod tests;
