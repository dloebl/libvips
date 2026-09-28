//! Errors, and the guard every callback from C goes through.
//!
//! libvips reports errors by appending to its error buffer with
//! `vips_error()` and returning `-1` (or `NULL`). Rust code returns
//! [`Result`] instead, and [`guard`] does the translation at the FFI
//! boundary.
//!
//! A panic must not unwind into C (Rust aborts the process if it tries), so
//! the guards also catch panics and report them as errors. The operation
//! fails, the process keeps running.

use std::any::Any;
use std::ffi::{c_int, CStr, CString};
use std::fmt;
use std::panic::{self, AssertUnwindSafe};

use crate::sys;

#[derive(Debug)]
pub enum Error {
    /// A libvips call failed, and has already put the details in the vips
    /// error buffer.
    Vips,
    /// An error to report.
    Message(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error::Message(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Vips => f.write_str("libvips error"),
            Error::Message(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Error::Message(error.to_string())
    }
}

/// Turn the `int` result of a libvips call into a [`Result`].
pub fn check(result: c_int) -> Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(Error::Vips)
    }
}

/// Add `error` to the vips error buffer.
pub fn report(domain: &CStr, error: &Error) {
    if let Error::Message(message) = error {
        // interior NULs would truncate the message, drop them
        let message = CString::new(message.replace('\0', "")).unwrap_or_default();

        // SAFETY: both strings are NUL-terminated, and "%s" consumes
        // exactly one string argument
        unsafe {
            sys::vips_error(domain.as_ptr(), c"%s".as_ptr(), message.as_ptr());
        }
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<&str>() {
        message
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message
    } else {
        "unknown panic"
    }
}

/// Run `f`, reporting any error or panic to the vips error buffer under
/// `domain`. `None` means `f` failed.
pub fn catch<T>(domain: &CStr, f: impl FnOnce() -> Result<T>) -> Option<T> {
    // A panicking operation can leave its own state inconsistent, but it
    // is failed and never used again, so there is nothing to observe.
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(value)) => Some(value),
        Ok(Err(error)) => {
            report(domain, &error);
            None
        }
        Err(payload) => {
            let message = format!("internal error: {}", panic_message(&*payload));
            report(domain, &Error::Message(message));
            None
        }
    }
}

/// [`catch`] for callbacks returning the usual `0` on success, `-1` on error.
pub fn guard(domain: &CStr, f: impl FnOnce() -> Result<()>) -> c_int {
    match catch(domain, f) {
        Some(()) => 0,
        None => -1,
    }
}

/// [`catch`] for callbacks that can't report an error, like `is_a()`: a
/// panic is swallowed and `default` returned.
pub fn guard_silent<T>(default: T, f: impl FnOnce() -> T) -> T {
    panic::catch_unwind(AssertUnwindSafe(f)).unwrap_or(default)
}
