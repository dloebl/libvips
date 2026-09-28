//! `VipsImage`.

use std::ffi::c_int;
use std::ptr::NonNull;

use crate::error::{check, Error, Result};
use crate::sys;

/// A reference to a `VipsImage`, dropped with `g_object_unref()`.
#[derive(Debug)]
pub struct Image(NonNull<sys::VipsImage>);

// SAFETY: GObject reference counting is thread-safe, and libvips shares
// images between its worker threads
unsafe impl Send for Image {}
// SAFETY: as above, &Image only reads header fields
unsafe impl Sync for Image {}

impl Image {
    /// A new, empty image, ready for [`Image::init_fields`] and
    /// [`Image::generate`] (`vips_image_new()`).
    pub fn new() -> Result<Image> {
        // SAFETY: returns a new reference, or NULL
        unsafe { Image::from_owned(sys::vips_image_new()) }.ok_or(Error::Vips)
    }

    /// Take over a reference, eg. the result of `vips_image_new()`.
    ///
    /// # Safety
    /// `image` is NULL or a `VipsImage` the caller owns a reference to.
    pub unsafe fn from_owned(image: *mut sys::VipsImage) -> Option<Image> {
        NonNull::new(image).map(Image)
    }

    /// Add a reference to an image owned by someone else, eg. an argument.
    ///
    /// # Safety
    /// `image` is NULL or a live `VipsImage`.
    pub unsafe fn from_borrowed(image: *mut sys::VipsImage) -> Option<Image> {
        let image = NonNull::new(image)?;
        sys::g_object_ref(image.as_ptr() as *mut _);
        Some(Image(image))
    }

    pub fn as_ptr(&self) -> *mut sys::VipsImage {
        self.0.as_ptr()
    }

    fn raw(&self) -> &sys::VipsImage {
        // SAFETY: we hold a reference
        unsafe { self.0.as_ref() }
    }

    pub fn width(&self) -> c_int {
        self.raw().Xsize
    }

    pub fn height(&self) -> c_int {
        self.raw().Ysize
    }

    pub fn bands(&self) -> c_int {
        self.raw().Bands
    }

    pub fn format(&self) -> sys::VipsBandFormat {
        self.raw().BandFmt
    }

    pub fn coding(&self) -> sys::VipsCoding {
        self.raw().Coding
    }

    pub fn interpretation(&self) -> sys::VipsInterpretation {
        self.raw().Type
    }

    /// Bytes per pixel, `VIPS_IMAGE_SIZEOF_PEL()`.
    pub fn sizeof_pel(&self) -> usize {
        // SAFETY: plain lookup, 0 for invalid formats
        let sizeof_element = unsafe { sys::vips_format_sizeof(self.format()) } as usize;
        sizeof_element * self.bands().max(0) as usize
    }

    /// Bytes per line, `VIPS_IMAGE_SIZEOF_LINE()`.
    pub fn sizeof_line(&self) -> usize {
        self.sizeof_pel() * self.width().max(0) as usize
    }

    /// Set the header fields (`vips_image_init_fields()`).
    #[allow(clippy::too_many_arguments)]
    pub fn init_fields(
        &self,
        width: c_int,
        height: c_int,
        bands: c_int,
        format: sys::VipsBandFormat,
        coding: sys::VipsCoding,
        interpretation: sys::VipsInterpretation,
        xres: f64,
        yres: f64,
    ) {
        // SAFETY: plain field assignments
        unsafe {
            sys::vips_image_init_fields(
                self.as_ptr(),
                width,
                height,
                bands,
                format,
                coding,
                interpretation,
                xres,
                yres,
            );
        }
    }

    /// Link this image into a pipeline after `inputs`, copying their header
    /// fields, and set the demand hint (`vips_image_pipeline_array()`).
    pub fn pipeline(&self, style: sys::VipsDemandStyle, inputs: &[&Image]) -> Result<()> {
        let mut array: Vec<*mut sys::VipsImage> = inputs.iter().map(|i| i.as_ptr()).collect();
        array.push(std::ptr::null_mut());

        // SAFETY: a NULL-terminated array of live images
        check(unsafe { sys::vips_image_pipeline_array(self.as_ptr(), style, array.as_mut_ptr()) })
    }

    /// Write line `y` of an image being written line by line
    /// (`vips_image_write_line()`). `line` must hold exactly one line.
    pub fn write_line(&self, y: c_int, line: &[u8]) -> Result<()> {
        if y < 0 || y >= self.height() || line.len() != self.sizeof_line() {
            return Err(Error::new("write_line: bad line"));
        }

        // SAFETY: line is a whole line, and vips only reads it
        check(unsafe { sys::vips_image_write_line(self.as_ptr(), y, line.as_ptr() as *mut _) })
    }
}

impl Clone for Image {
    fn clone(&self) -> Self {
        // SAFETY: we hold a reference, so it's live
        unsafe { Image::from_borrowed(self.as_ptr()) }.unwrap()
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: we own one reference
        unsafe { sys::g_object_unref(self.as_ptr() as *mut _) };
    }
}

/// Rust types for the elements of each band format.
///
/// # Safety
/// `Self` must have the size and alignment of `FORMAT` elements, and be
/// valid for any bit pattern.
pub unsafe trait Pixel: Copy + Send + Sync + 'static {
    const FORMAT: sys::VipsBandFormat;
}

macro_rules! pixel {
    ($($type:ty => $format:ident),*) => {$(
        // SAFETY: $type is the element type of $format, valid for any bits
        unsafe impl Pixel for $type {
            const FORMAT: sys::VipsBandFormat = sys::$format;
        }
    )*};
}

pixel!(
    u8 => VIPS_FORMAT_UCHAR,
    i8 => VIPS_FORMAT_CHAR,
    u16 => VIPS_FORMAT_USHORT,
    i16 => VIPS_FORMAT_SHORT,
    u32 => VIPS_FORMAT_UINT,
    i32 => VIPS_FORMAT_INT,
    f32 => VIPS_FORMAT_FLOAT,
    f64 => VIPS_FORMAT_DOUBLE,
    [f32; 2] => VIPS_FORMAT_COMPLEX,
    [f64; 2] => VIPS_FORMAT_DPCOMPLEX
);
