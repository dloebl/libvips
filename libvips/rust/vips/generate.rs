//! The start / generate / stop callbacks of `vips_image_generate()`.
//!
//! libvips computes images on demand, in parallel: every worker thread calls
//! `start` once to make its sequence (usually regions on the inputs), then
//! `generate` for each area it needs, then `stop`. A [`Generate`] value is
//! shared by all threads, the sequence belongs to one.
//!
//! ```ignore
//! struct Invert { input: Image }
//!
//! impl Generate for Invert {
//!     type Seq = Region;
//!
//!     fn start(&self) -> Result<Region> {
//!         Region::new(&self.input)
//!     }
//!
//!     fn generate(&self, input: &mut Region, out: &mut OutRegion) -> Result<()> {
//!         input.prepare(&out.valid())?;
//!         out.for_each_line(|y, line: &mut [u8]| {
//!             for (o, i) in line.iter_mut().zip(input.line::<u8>(y)) {
//!                 *o = 255 - i;
//!             }
//!         });
//!         Ok(())
//!     }
//! }
//!
//! out.generate(Invert { input })?;
//! ```

use std::ffi::{c_int, c_void, CStr};

use crate::error::{self, check, Error, Result};
use crate::image::Image;
use crate::region::OutRegion;
use crate::sys;

pub trait Generate: Send + Sync + Sized + 'static {
    /// Per-thread state.
    type Seq: Send + 'static;

    /// Error domain for failures in the callbacks.
    const DOMAIN: &'static CStr = c"vips";

    fn start(&self) -> Result<Self::Seq>;

    /// Compute `out.valid()`.
    fn generate(&self, seq: &mut Self::Seq, out: &mut OutRegion<'_>) -> Result<()>;
}

/// The key the generator is stored under on the image.
const KEY: &CStr = c"vips-rust-generate";

impl Image {
    /// Compute this image with `generator` (`vips_image_generate()`).
    ///
    /// The image keeps `generator` until it is finalized, after all its
    /// regions, and so all callbacks, are gone.
    pub fn generate<G: Generate>(&self, generator: G) -> Result<()> {
        let object = self.as_ptr() as *mut sys::GObject;

        // SAFETY: a plain lookup on a live object
        if !unsafe { sys::g_object_get_data(object, KEY.as_ptr()) }.is_null() {
            return Err(Error::new("image already has a generate function"));
        }

        let generator = Box::into_raw(Box::new(generator)) as *mut c_void;

        // SAFETY: the image owns the box from here on, and frees it with
        // the matching type
        unsafe {
            sys::g_object_set_data_full(object, KEY.as_ptr(), generator, Some(drop_box::<G>));

            check(sys::vips_image_generate(
                self.as_ptr(),
                Some(start::<G>),
                Some(generate::<G>),
                Some(stop::<G>),
                generator,
                std::ptr::null_mut(),
            ))
        }
    }
}

unsafe extern "C" fn drop_box<G>(data: *mut c_void) {
    error::guard_silent((), || drop(Box::from_raw(data as *mut G)));
}

unsafe extern "C" fn start<G: Generate>(
    _out: *mut sys::VipsImage,
    a: *mut c_void,
    _b: *mut c_void,
) -> *mut c_void {
    let generator = &*(a as *const G);

    // NULL is a failed start. A Box is never NULL, even for zero-sized
    // sequences.
    error::catch(G::DOMAIN, || generator.start()).map_or(std::ptr::null_mut(), |seq| {
        Box::into_raw(Box::new(seq)) as *mut c_void
    })
}

unsafe extern "C" fn generate<G: Generate>(
    out: *mut sys::VipsRegion,
    seq: *mut c_void,
    a: *mut c_void,
    _b: *mut c_void,
    _stop: *mut sys::gboolean,
) -> c_int {
    let generator = &*(a as *const G);
    let seq = &mut *(seq as *mut G::Seq);
    let mut out = OutRegion::new(out);

    error::guard(G::DOMAIN, || generator.generate(seq, &mut out))
}

unsafe extern "C" fn stop<G: Generate>(
    seq: *mut c_void,
    _a: *mut c_void,
    _b: *mut c_void,
) -> c_int {
    error::guard(G::DOMAIN, || {
        drop(Box::from_raw(seq as *mut G::Seq));
        Ok(())
    })
}
