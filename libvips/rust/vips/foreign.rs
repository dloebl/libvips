//! Loaders: subclasses of `VipsForeignLoad`.
//!
//! `VipsForeignLoad` drives loading: it calls [`Load::header`] to get the
//! image header without decoding pixels, then [`Load::load`] when pixels
//! are needed.

use std::ffi::{c_char, c_void, CStr};
use std::path::PathBuf;

use crate::error::{self, Result};
use crate::image::Image;
use crate::object::{Class, IsForeignClass, ObjectSubclass, StaticType};
use crate::sys;

pub trait Load: ObjectSubclass<Class = sys::VipsForeignLoadClass> + StaticType {
    /// Set the header fields of `out`, without decoding pixels.
    fn header(&mut self, out: &Image) -> Result<()>;

    /// Set the header fields of `real` and fill it with pixels, eg. with
    /// [`Image::generate`] or [`Image::write_line`]. libvips copies it to
    /// the output.
    fn load(&mut self, real: &Image) -> Result<()>;

    /// `VipsForeignFlags` for this image, eg. `VIPS_FOREIGN_PARTIAL` if any
    /// area can be loaded quickly, so `real` can be generated on demand.
    fn flags(&self) -> sys::VipsForeignFlags {
        0
    }
}

/// Loaders that can sniff files.
pub trait IsAFile: Load {
    fn is_a(path: PathBuf) -> bool;
}

/// Loaders that can sniff memory buffers.
pub trait IsABuffer: Load {
    fn is_a_buffer(data: &[u8]) -> bool;
}

/// The bytes of a `VipsBlob` argument, eg. the "buffer" of a buffer loader.
///
/// # Safety
/// `blob` is NULL or a live `VipsBlob`, which outlives the result.
pub unsafe fn blob<'a>(blob: *mut sys::VipsBlob) -> Option<&'a [u8]> {
    if blob.is_null() {
        return None;
    }

    let mut length = 0;
    let data = sys::vips_blob_get(blob, &mut length) as *const u8;
    if data.is_null() {
        Some(&[])
    } else {
        Some(std::slice::from_raw_parts(data, length))
    }
}

/// A filename argument as a path.
///
/// # Safety
/// `filename` is NULL or a NUL-terminated string.
pub unsafe fn path(filename: *const c_char) -> Option<PathBuf> {
    if filename.is_null() {
        return None;
    }
    let filename = CStr::from_ptr(filename);

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(std::ffi::OsStr::from_bytes(filename.to_bytes()).into())
    }

    // libvips filenames are UTF-8 on other platforms
    #[cfg(not(unix))]
    {
        Some(filename.to_string_lossy().into_owned().into())
    }
}

impl<T: ObjectSubclass> Class<T>
where
    T::Class: IsForeignClass,
{
    /// Filename suffixes, eg. `&[c".bmp"]`.
    pub fn set_suffixes(&mut self, suffixes: &[&'static CStr]) {
        // a NULL-terminated array, leaked like the class itself
        let mut array: Vec<*const c_char> = suffixes.iter().map(|s| s.as_ptr()).collect();
        array.push(std::ptr::null());
        let array = Box::leak(array.into_boxed_slice());

        let foreign_class = self.raw_ptr() as *mut sys::VipsForeignClass;
        // SAFETY: the class starts with a VipsForeignClass, and the array
        // lives forever
        unsafe { (*foreign_class).suffs = array.as_mut_ptr() };
    }

    /// Loaders are tried in priority order, highest first.
    pub fn set_priority(&mut self, priority: i32) {
        let foreign_class = self.raw_ptr() as *mut sys::VipsForeignClass;
        // SAFETY: the class starts with a VipsForeignClass
        unsafe { (*foreign_class).priority = priority };
    }
}

impl<T: Load> Class<T> {
    /// Install the header, load and get_flags vfuncs.
    pub fn install_load(&mut self) {
        let class = self.raw_ptr();
        // SAFETY: the trampolines are called with instances of T
        unsafe {
            (*class).header = Some(header::<T>);
            (*class).load = Some(load::<T>);
            (*class).get_flags = Some(get_flags::<T>);
        }
    }
}

impl<T: IsAFile> Class<T> {
    pub fn install_is_a(&mut self) {
        let class = self.raw_ptr();
        // SAFETY: the trampoline only takes a filename
        unsafe { (*class).is_a = Some(is_a::<T>) };
    }
}

impl<T: IsABuffer> Class<T> {
    pub fn install_is_a_buffer(&mut self) {
        let class = self.raw_ptr();
        // SAFETY: the trampoline only takes a buffer
        unsafe { (*class).is_a_buffer = Some(is_a_buffer::<T>) };
    }
}

unsafe extern "C" fn header<T: Load>(load: *mut sys::VipsForeignLoad) -> i32 {
    error::guard(T::NICKNAME, || {
        let out = Image::from_borrowed((*load).out).ok_or(error::Error::Vips)?;
        T::header(&mut *(load as *mut T), &out)
    })
}

unsafe extern "C" fn load<T: Load>(load: *mut sys::VipsForeignLoad) -> i32 {
    error::guard(T::NICKNAME, || {
        let real = Image::from_borrowed((*load).real).ok_or(error::Error::Vips)?;
        T::load(&mut *(load as *mut T), &real)
    })
}

unsafe extern "C" fn get_flags<T: Load>(load: *mut sys::VipsForeignLoad) -> sys::VipsForeignFlags {
    error::guard_silent(0, || T::flags(&*(load as *const T)))
}

unsafe extern "C" fn is_a<T: IsAFile>(filename: *const c_char) -> sys::gboolean {
    error::guard_silent(0, || match path(filename) {
        Some(path) => T::is_a(path) as sys::gboolean,
        None => 0,
    })
}

unsafe extern "C" fn is_a_buffer<T: IsABuffer>(data: *const c_void, size: usize) -> sys::gboolean {
    error::guard_silent(0, || {
        let data = if data.is_null() {
            &[]
        } else {
            std::slice::from_raw_parts(data as *const u8, size)
        };
        T::is_a_buffer(data) as sys::gboolean
    })
}
