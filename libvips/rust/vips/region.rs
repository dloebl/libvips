//! `VipsRegion`: pixel access, bounds- and type-checked.
//!
//! Rows are handed out as slices of [`Pixel`], so reading or writing
//! outside the valid area, or with the wrong element type, panics (and the
//! guard turns that into an error) instead of corrupting memory.

use std::ffi::c_int;
use std::marker::PhantomData;
use std::ptr::NonNull;

use crate::error::{check, Error, Result};
use crate::image::{Image, Pixel};
use crate::sys;

/// A rectangle, `VipsRect`.
pub type Rect = sys::VipsRect;

/// `VIPS_RECT_BOTTOM()` and friends.
pub trait RectExt {
    fn right(&self) -> c_int;
    fn bottom(&self) -> c_int;
    fn is_empty(&self) -> bool;
    fn contains(&self, other: &Rect) -> bool;
}

impl RectExt for Rect {
    fn right(&self) -> c_int {
        self.left + self.width
    }

    fn bottom(&self) -> c_int {
        self.top + self.height
    }

    fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    fn contains(&self, other: &Rect) -> bool {
        other.left >= self.left
            && other.top >= self.top
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }
}

/// Where the pixels of line `y` of a region start, and how many elements
/// of `P` it has.
///
/// # Safety
/// `region` is live and its `valid` area is backed by memory.
unsafe fn line<P: Pixel>(region: *mut sys::VipsRegion, y: c_int) -> (*mut P, usize) {
    let region = &*region;
    let valid = &region.valid;
    assert!(
        y >= valid.top && y < valid.bottom(),
        "line {y} outside the region"
    );

    // SAFETY: regions always belong to a live image
    let image = &*region.im;
    assert!(
        image.BandFmt == P::FORMAT,
        "pixel type does not match the image format"
    );

    let offset = (y - valid.top) as isize * region.bpl as isize;
    let data = region.data.offset(offset) as *mut P;
    assert!(
        data as usize % std::mem::align_of::<P>() == 0,
        "misaligned region"
    );

    (data, valid.width as usize * image.Bands as usize)
}

/// A region on an input image, `vips_region_new()`. Create one per thread,
/// in [`Generate::start`](crate::Generate::start).
#[derive(Debug)]
pub struct Region(NonNull<sys::VipsRegion>);

// SAFETY: a region can move between threads, it just can't be shared
unsafe impl Send for Region {}

impl Region {
    pub fn new(image: &Image) -> Result<Region> {
        // SAFETY: the image is live
        let region = unsafe { sys::vips_region_new(image.as_ptr()) };
        NonNull::new(region).map(Region).ok_or(Error::Vips)
    }

    /// Compute the pixels in `rect`, `vips_region_prepare()`.
    pub fn prepare(&mut self, rect: &Rect) -> Result<()> {
        // SAFETY: we own the region
        check(unsafe { sys::vips_region_prepare(self.0.as_ptr(), rect) })
    }

    /// The area [`Region::prepare`] made available.
    pub fn valid(&self) -> Rect {
        // SAFETY: we own the region
        unsafe { self.0.as_ref().valid }
    }

    /// Line `y` of the valid area, from `valid().left`.
    pub fn line<P: Pixel>(&self, y: c_int) -> &[P] {
        // SAFETY: after a successful prepare(), valid is backed by memory
        // that stays put until the next prepare(), which needs &mut self
        unsafe {
            let (data, len) = line::<P>(self.0.as_ptr(), y);
            std::slice::from_raw_parts(data, len)
        }
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        // SAFETY: we own the region
        unsafe { sys::g_object_unref(self.0.as_ptr() as *mut _) };
    }
}

/// The output region of a generate callback. Its valid area is the area to
/// compute.
pub struct OutRegion<'a> {
    region: NonNull<sys::VipsRegion>,
    _marker: PhantomData<&'a mut sys::VipsRegion>,
}

impl OutRegion<'_> {
    /// # Safety
    /// `region` is the output region of a generate callback, and outlives
    /// the returned value.
    pub(crate) unsafe fn new<'a>(region: *mut sys::VipsRegion) -> OutRegion<'a> {
        OutRegion {
            region: NonNull::new(region).expect("NULL output region"),
            _marker: PhantomData,
        }
    }

    pub fn valid(&self) -> Rect {
        // SAFETY: the region outlives self
        unsafe { self.region.as_ref().valid }
    }

    /// Line `y` of the area to compute.
    pub fn line_mut<P: Pixel>(&mut self, y: c_int) -> &mut [P] {
        // SAFETY: vips gives the calling thread exclusive use of the
        // output region's memory, and &mut self stops overlapping borrows
        unsafe {
            let (data, len) = line::<P>(self.region.as_ptr(), y);
            std::slice::from_raw_parts_mut(data, len)
        }
    }

    /// Call `f` with the y coordinate and contents of every line.
    pub fn for_each_line<P: Pixel>(&mut self, mut f: impl FnMut(c_int, &mut [P])) {
        let valid = self.valid();
        for y in valid.top..valid.bottom() {
            f(y, self.line_mut(y));
        }
    }
}
