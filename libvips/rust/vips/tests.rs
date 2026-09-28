//! Tests against the real libvips, run with "meson test".

use std::ffi::{c_char, c_int, CStr};
use std::ptr;
use std::sync::{Mutex, MutexGuard, Once};

use crate::object::{set_output_image, Build};
use crate::region::RectExt;
use crate::*;

/// Init libvips once, and serialise tests: the vips error buffer is global.
fn setup() -> MutexGuard<'static, ()> {
    static INIT: Once = Once::new();
    static LOCK: Mutex<()> = Mutex::new(());

    INIT.call_once(|| {
        // SAFETY: a static C string
        assert_eq!(unsafe { sys::vips_init(c"vips-rust-test".as_ptr()) }, 0);
    });

    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: plain call
    unsafe { sys::vips_error_clear() };
    guard
}

fn take_error() -> String {
    // SAFETY: the buffer is a C string owned by vips
    unsafe {
        let error = CStr::from_ptr(sys::vips_error_buffer())
            .to_string_lossy()
            .into_owned();
        sys::vips_error_clear();
        error
    }
}

fn rect(left: c_int, top: c_int, width: c_int, height: c_int) -> Rect {
    Rect {
        left,
        top,
        width,
        height,
    }
}

/// A one-band image computed by `f(x, y)`.
struct Pattern<P, F> {
    f: F,
    _pixel: std::marker::PhantomData<P>,
}

impl<P: Pixel, F: Fn(c_int, c_int) -> P + Send + Sync + 'static> Generate for Pattern<P, F> {
    type Seq = ();

    fn start(&self) -> Result<()> {
        Ok(())
    }

    fn generate(&self, _: &mut (), out: &mut OutRegion<'_>) -> Result<()> {
        let left = out.valid().left;
        out.for_each_line(|y, line: &mut [P]| {
            for (i, pixel) in line.iter_mut().enumerate() {
                *pixel = (self.f)(left + i as c_int, y);
            }
        });
        Ok(())
    }
}

fn pattern<P: Pixel>(
    width: c_int,
    height: c_int,
    f: impl Fn(c_int, c_int) -> P + Send + Sync + 'static,
) -> Image {
    let image = Image::new().unwrap();
    image.init_fields(
        width,
        height,
        1,
        P::FORMAT,
        sys::VIPS_CODING_NONE,
        sys::VIPS_INTERPRETATION_MULTIBAND,
        1.0,
        1.0,
    );
    image.pipeline(sys::VIPS_DEMAND_STYLE_ANY, &[]).unwrap();
    image
        .generate(Pattern {
            f,
            _pixel: std::marker::PhantomData,
        })
        .unwrap();
    image
}

#[test]
fn generate_and_read_back() {
    let _lock = setup();
    let image = pattern(64, 32, |x, y| (x + 2 * y) as u8);
    assert_eq!((image.width(), image.height(), image.bands()), (64, 32, 1));

    let mut region = Region::new(&image).unwrap();
    let area = rect(10, 5, 20, 7);
    region.prepare(&area).unwrap();
    assert!(region.valid().contains(&area));

    for y in area.top..area.bottom() {
        let line = region.line::<u8>(y);
        assert_eq!(line.len(), 20);
        for (i, &pixel) in line.iter().enumerate() {
            assert_eq!(pixel, (10 + i as c_int + 2 * y) as u8);
        }
    }
}

#[test]
fn whole_image_through_libvips() {
    let _lock = setup();
    let image = pattern(300, 200, |x, y| (x * y) as f32);

    // a real vips pipeline, run on the vips threadpool
    let mut size = 0;
    // SAFETY: returns a g_malloc()ed buffer of size bytes, or NULL
    let data = unsafe { sys::vips_image_write_to_memory(image.as_ptr(), &mut size) };
    assert!(!data.is_null(), "{}", take_error());
    assert_eq!(size, 300 * 200 * 4);

    // SAFETY: data holds size bytes of floats
    let pixels = unsafe { std::slice::from_raw_parts(data as *const f32, 300 * 200) };
    for y in 0..200 {
        for x in 0..300 {
            assert_eq!(pixels[(y * 300 + x) as usize], (x * y) as f32);
        }
    }
    // SAFETY: allocated by vips with g_malloc()
    unsafe { sys::g_free(data) };
}

#[test]
fn wrong_pixel_type_panics() {
    let _lock = setup();
    let image = pattern(8, 8, |_, _| 0u16);
    let mut region = Region::new(&image).unwrap();
    region.prepare(&rect(0, 0, 8, 8)).unwrap();

    assert_eq!(region.line::<u16>(0).len(), 8);
    let result = std::panic::catch_unwind(|| region.line::<u8>(0).len());
    assert!(result.is_err());
    let result = std::panic::catch_unwind(|| region.line::<u16>(8).len());
    assert!(result.is_err());
}

struct Failing {
    panic: bool,
}

impl Generate for Failing {
    type Seq = ();
    const DOMAIN: &'static CStr = c"failing";

    fn start(&self) -> Result<()> {
        Ok(())
    }

    fn generate(&self, _: &mut (), _: &mut OutRegion<'_>) -> Result<()> {
        if self.panic {
            panic!("boom");
        }
        Err(Error::new("nope"))
    }
}

fn failing(panic: bool) -> Image {
    let image = Image::new().unwrap();
    image.init_fields(
        16,
        16,
        1,
        sys::VIPS_FORMAT_UCHAR,
        sys::VIPS_CODING_NONE,
        sys::VIPS_INTERPRETATION_B_W,
        1.0,
        1.0,
    );
    image.pipeline(sys::VIPS_DEMAND_STYLE_ANY, &[]).unwrap();
    image.generate(Failing { panic }).unwrap();
    image
}

#[test]
fn errors_are_reported() {
    let _lock = setup();
    let mut region = Region::new(&failing(false)).unwrap();
    assert!(region.prepare(&rect(0, 0, 4, 4)).is_err());
    assert_eq!(take_error(), "failing: nope\n");
}

#[test]
fn panics_become_errors() {
    let _lock = setup();
    let mut region = Region::new(&failing(true)).unwrap();
    assert!(region.prepare(&rect(0, 0, 4, 4)).is_err());
    assert_eq!(take_error(), "failing: internal error: boom\n");

    // and libvips keeps working
    let image = pattern(4, 4, |x, _| x as u8);
    let mut region = Region::new(&image).unwrap();
    region.prepare(&rect(0, 0, 4, 4)).unwrap();
    assert_eq!(region.line::<u8>(3), &[0, 1, 2, 3]);
}

#[test]
fn generate_twice_fails() {
    let _lock = setup();
    let image = pattern(4, 4, |_, _| 0u8);
    let again = image.generate(Pattern {
        f: |_, _| 0u8,
        _pixel: std::marker::PhantomData,
    });
    assert!(again.is_err());
}

/// A test operation: out = in + c, for uchar images.
#[repr(C)]
struct AddConst {
    parent: sys::VipsOperation,
    input: *mut sys::VipsImage,
    output: *mut sys::VipsImage,
    c: c_int,
}

// SAFETY: repr(C), starts with its parent, valid zeroed, owns nothing
unsafe impl ObjectSubclass for AddConst {
    const NAME: &'static CStr = c"VipsRustTestAdd";
    const NICKNAME: &'static CStr = c"rust_test_add";
    const DESCRIPTION: &'static CStr = c"add a constant to a uchar image";
    type Class = sys::VipsOperationClass;

    fn parent_type() -> sys::GType {
        // SAFETY: plain type lookup
        unsafe { sys::vips_operation_get_type() }
    }

    fn class_init(class: &mut Class<Self>) {
        class.install_build();
        class.add_operation_flags(sys::VIPS_OPERATION_SEQUENTIAL);
        class.arg_image(
            Arg::required_input(c"in", 1, c"Input", c"Input image"),
            field!(AddConst, input),
        );
        class.arg_image(
            Arg::required_output(c"out", 2, c"Output", c"Output image"),
            field!(AddConst, output),
        );
        class.arg_int(
            Arg::optional_input(c"c", 3, c"C", c"Constant to add"),
            field!(AddConst, c),
            0,
            255,
            1,
        );
    }

    fn instance_init(&mut self) {
        self.c = 1;
    }
}

object_type!(vips_rust_test_add_get_type, AddConst);

struct AddGen {
    input: Image,
    c: u8,
}

impl Generate for AddGen {
    type Seq = Region;
    const DOMAIN: &'static CStr = AddConst::NICKNAME;

    fn start(&self) -> Result<Region> {
        Region::new(&self.input)
    }

    fn generate(&self, input: &mut Region, out: &mut OutRegion<'_>) -> Result<()> {
        input.prepare(&out.valid())?;
        out.for_each_line(|y, line: &mut [u8]| {
            for (o, i) in line.iter_mut().zip(input.line::<u8>(y)) {
                *o = i.saturating_add(self.c);
            }
        });
        Ok(())
    }
}

impl Build for AddConst {
    fn build(&mut self) -> Result<()> {
        // SAFETY: a required input, so set and live
        let input = unsafe { Image::from_borrowed(self.input) }.ok_or(Error::Vips)?;
        if input.format() != sys::VIPS_FORMAT_UCHAR || input.coding() != sys::VIPS_CODING_NONE {
            return Err(Error::new("uchar images only"));
        }

        let out = set_output_image(self, c"out", Image::new()?)?;
        out.pipeline(sys::VIPS_DEMAND_STYLE_THINSTRIP, &[&input])?;
        out.generate(AddGen {
            input,
            c: self.c as u8,
        })
    }
}

/// Call rust_test_add through the C API, the way bindings do.
fn call_add(input: &Image, c: Option<c_int>) -> Option<Image> {
    assert_ne!(vips_rust_test_add_get_type(), 0);

    let mut out: *mut sys::VipsImage = ptr::null_mut();
    // SAFETY: vips_call() takes the required arguments, then
    // NULL-terminated name / value pairs
    let result = unsafe {
        match c {
            Some(c) => sys::vips_call(
                c"rust_test_add".as_ptr(),
                input.as_ptr(),
                &mut out,
                c"c".as_ptr(),
                c,
                ptr::null::<c_char>(),
            ),
            None => sys::vips_call(
                c"rust_test_add".as_ptr(),
                input.as_ptr(),
                &mut out,
                ptr::null::<c_char>(),
            ),
        }
    };

    // SAFETY: on success, out is a new reference
    (result == 0).then(|| unsafe { Image::from_owned(out) }.unwrap())
}

#[test]
fn operation() {
    let _lock = setup();
    let input = pattern(100, 50, |x, y| (x + y) as u8);

    for (c, expected) in [(None, 1u8), (Some(7), 7)] {
        let out = call_add(&input, c).unwrap_or_else(|| panic!("{}", take_error()));
        assert_eq!((out.width(), out.height()), (100, 50));

        let mut region = Region::new(&out).unwrap();
        region.prepare(&rect(0, 0, 100, 50)).unwrap();
        for y in 0..50 {
            for (x, &pixel) in region.line::<u8>(y).iter().enumerate() {
                assert_eq!(pixel, (x as c_int + y) as u8 + expected);
            }
        }
    }
}

#[test]
fn operation_errors() {
    let _lock = setup();
    let floats = pattern(10, 10, |_, _| 0f32);
    assert!(call_add(&floats, None).is_none());
    assert_eq!(take_error(), "rust_test_add: uchar images only\n");
}

#[test]
fn operation_is_introspectable() {
    let _lock = setup();
    assert_ne!(vips_rust_test_add_get_type(), 0);

    // SAFETY: static C strings
    let found =
        unsafe { sys::vips_type_find(c"VipsOperation".as_ptr(), c"rust_test_add".as_ptr()) };
    assert_eq!(found, vips_rust_test_add_get_type());

    // SAFETY: returns a static string
    let description = unsafe {
        let class = sys::g_type_class_ref(found) as *mut sys::VipsObjectClass;
        CStr::from_ptr((*class).description)
    };
    assert_eq!(description, AddConst::DESCRIPTION);
}
