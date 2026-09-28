//! GObject subclasses: type registration, arguments, `build`.
//!
//! A vips operation is an instance struct plus a class struct, registered
//! with GObject. In Rust:
//!
//! ```ignore
//! #[repr(C)]
//! pub struct Invert {
//!     parent: sys::VipsOperation,     // parent instance, must come first
//!     input: *mut sys::VipsImage,     // arguments, managed by VipsObject
//!     output: *mut sys::VipsImage,
//! }
//!
//! unsafe impl ObjectSubclass for Invert {
//!     const NAME: &'static CStr = c"VipsRustInvert";
//!     const NICKNAME: &'static CStr = c"rust_invert";
//!     const DESCRIPTION: &'static CStr = c"invert an image";
//!     type Class = sys::VipsOperationClass;   // parent class, reused as is
//!
//!     fn parent_type() -> sys::GType { unsafe { sys::vips_operation_get_type() } }
//!
//!     fn class_init(class: &mut Class<Self>) {
//!         class.install_build();
//!         class.arg_image(Arg::required_input(c"in", 1, c"Input", c"Input image"),
//!             field!(Invert, input));
//!         class.arg_image(Arg::required_output(c"out", 2, c"Output", c"Output image"),
//!             field!(Invert, output));
//!     }
//! }
//!
//! object_type!(vips_rust_invert_get_type, Invert);
//! ```

use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::marker::PhantomData;
use std::mem::size_of;

use crate::error::{self, Error, Result};
use crate::image::Image;
use crate::sys;

/// A vips class struct, which a subclass reuses unchanged as its own.
///
/// # Safety
///
/// Implementors must be `#[repr(C)]` vips class structs, starting with a
/// `VipsObjectClass`. The marker subtraits say which other class structs
/// they start with.
pub unsafe trait IsClass: Sized {}
/// # Safety
/// Starts with a `VipsOperationClass`.
pub unsafe trait IsOperationClass: IsClass {}
/// # Safety
/// Starts with a `VipsForeignClass`.
pub unsafe trait IsForeignClass: IsOperationClass {}

macro_rules! is_class {
    ($class:ty: $($marker:ident),*) => {$(
        // SAFETY: bindgen generated $class from the C struct, which
        // starts with the parent class structs
        unsafe impl $marker for $class {}
    )*};
}

is_class!(sys::VipsObjectClass: IsClass);
is_class!(sys::VipsOperationClass: IsClass, IsOperationClass);
is_class!(sys::VipsForeignClass: IsClass, IsOperationClass, IsForeignClass);
is_class!(sys::VipsForeignLoadClass: IsClass, IsOperationClass, IsForeignClass);
is_class!(sys::VipsForeignSaveClass: IsClass, IsOperationClass, IsForeignClass);

/// An instance struct registered as a GObject subclass.
///
/// # Safety
///
/// - `Self` is `#[repr(C)]` and its first field is the instance struct of
///   [`parent_type`](Self::parent_type)
/// - [`Class`](Self::Class) is the class struct of `parent_type`
/// - all-zero bytes are a valid `Self`, since GObject zero-fills new
///   instances: use raw pointers, integers and `Option<Box<_>>` /
///   `Option<Arc<_>>`, not references or `Vec`
/// - fields that own memory are freed in [`finalize`](Self::finalize)
pub unsafe trait ObjectSubclass: Sized + 'static {
    /// The GType name, eg. `c"VipsForeignLoadBmpFile"`.
    const NAME: &'static CStr;
    /// The vips nickname, eg. `c"bmpload"`, also used as the error domain.
    const NICKNAME: &'static CStr;
    const DESCRIPTION: &'static CStr;
    const ABSTRACT: bool = false;

    type Class: IsClass;

    fn parent_type() -> sys::GType;

    /// Install arguments and vfuncs. Nickname and description are already
    /// set.
    fn class_init(class: &mut Class<Self>);

    /// Set defaults on a new, zeroed instance. vips does not apply the
    /// argument defaults given in `class_init`, so optional arguments
    /// with a non-zero default must be set here.
    fn instance_init(&mut self) {}

    /// Drop fields that own memory, called when the object is finalized.
    fn finalize(&mut self) {}
}

/// The registered GType of a subclass, implemented by [`object_type!`].
///
/// # Safety
///
/// `static_type()` must return the type registered for `Self` by
/// [`register`].
pub unsafe trait StaticType {
    fn static_type() -> sys::GType;
}

/// Define the C `*_get_type()` function for an [`ObjectSubclass`], which
/// registers the type on first call.
///
/// ```ignore
/// object_type!(vips_foreign_load_bmp_file_get_type, BmpLoadFile);
/// ```
#[macro_export]
macro_rules! object_type {
    ($get_type:ident, $type:ty) => {
        #[no_mangle]
        pub extern "C" fn $get_type() -> $crate::sys::GType {
            static TYPE: ::std::sync::OnceLock<$crate::sys::GType> = ::std::sync::OnceLock::new();

            *TYPE.get_or_init($crate::object::register::<$type>)
        }

        // SAFETY: returns the type registered for $type
        unsafe impl $crate::object::StaticType for $type {
            fn static_type() -> $crate::sys::GType {
                $get_type()
            }
        }
    };
}

/// Register `T` with GObject. Use [`object_type!`] rather than calling this
/// directly. Returns 0 (`G_TYPE_INVALID`) on failure.
pub fn register<T: ObjectSubclass + StaticType>() -> sys::GType {
    error::catch(T::NICKNAME, || {
        // SAFETY: GTypeQuery is plain data, and g_type_query() fills it
        let parent = unsafe {
            let mut query: sys::GTypeQuery = std::mem::zeroed();
            sys::g_type_query(T::parent_type(), &mut query);
            query
        };

        // catch a wrong Class type or a missing parent field
        if parent.type_ == 0
            || parent.class_size as usize != size_of::<T::Class>()
            || (parent.instance_size as usize) > size_of::<T>()
            || size_of::<T>() > u16::MAX as usize
        {
            return Err(Error::new(format!(
                "{}: class or instance struct does not match the parent type",
                T::NAME.to_string_lossy()
            )));
        }

        let info = sys::GTypeInfo {
            class_size: size_of::<T::Class>() as u16,
            base_init: None,
            base_finalize: None,
            class_init: Some(class_init_trampoline::<T>),
            class_finalize: None,
            class_data: std::ptr::null(),
            instance_size: size_of::<T>() as u16,
            n_preallocs: 0,
            instance_init: Some(instance_init_trampoline::<T>),
            value_table: std::ptr::null(),
        };
        let flags = if T::ABSTRACT {
            sys::G_TYPE_FLAG_ABSTRACT
        } else {
            0
        };

        // SAFETY: info is complete, and the name is a static C string
        let gtype = unsafe {
            sys::g_type_register_static(T::parent_type(), T::NAME.as_ptr(), &info, flags)
        };
        if gtype == 0 {
            return Err(Error::Vips);
        }

        Ok(gtype)
    })
    .unwrap_or(0)
}

/// The class struct of the parent of `T`, for chaining up.
fn parent_class<T: StaticType>() -> *mut c_void {
    // SAFETY: T is registered, so its class exists
    unsafe { sys::g_type_class_peek_parent(sys::g_type_class_peek(T::static_type())) }
}

unsafe extern "C" fn class_init_trampoline<T: ObjectSubclass + StaticType>(
    class: *mut c_void,
    _class_data: *mut c_void,
) {
    let gobject_class = class as *mut sys::GObjectClass;
    let object_class = class as *mut sys::VipsObjectClass;

    // GObject does not inherit these into subclasses
    (*gobject_class).set_property = Some(sys::vips_object_set_property);
    (*gobject_class).get_property = Some(sys::vips_object_get_property);
    (*gobject_class).finalize = Some(finalize_trampoline::<T>);

    (*object_class).nickname = T::NICKNAME.as_ptr();
    (*object_class).description = T::DESCRIPTION.as_ptr();

    let mut class = Class::<T> {
        raw: class as *mut T::Class,
        _marker: PhantomData,
    };
    error::catch(T::NICKNAME, || {
        T::class_init(&mut class);
        Ok(())
    });
}

unsafe extern "C" fn instance_init_trampoline<T: ObjectSubclass>(
    instance: *mut sys::GTypeInstance,
    _class: *mut c_void,
) {
    error::guard_silent((), || T::instance_init(&mut *(instance as *mut T)));
}

unsafe extern "C" fn finalize_trampoline<T: ObjectSubclass + StaticType>(
    object: *mut sys::GObject,
) {
    error::guard_silent((), || T::finalize(&mut *(object as *mut T)));

    let parent = parent_class::<T>() as *mut sys::GObjectClass;
    if let Some(finalize) = (*parent).finalize {
        finalize(object);
    }
}

/// The byte offset of a field of type `F` in instance struct `T`, made by
/// [`field!`].
pub struct Field<T, F> {
    offset: usize,
    _marker: PhantomData<fn(&T) -> &F>,
}

impl<T, F> Field<T, F> {
    #[doc(hidden)]
    pub unsafe fn __new(offset: usize, _type_of: fn(&T) -> &F) -> Self {
        Field {
            offset,
            _marker: PhantomData,
        }
    }
}

/// The [`Field`] for `Type.field`, eg. `field!(BmpLoadFile, filename)`.
#[macro_export]
macro_rules! field {
    ($type:ty, $field:ident) => {
        // SAFETY: the offset and the type are both taken from $type.$field
        unsafe {
            $crate::object::Field::<$type, _>::__new(
                ::std::mem::offset_of!($type, $field),
                |object: &$type| &object.$field,
            )
        }
    };
}

/// Name, docs, priority and flags of an argument.
#[derive(Clone, Copy)]
pub struct Arg {
    pub name: &'static CStr,
    pub nick: &'static CStr,
    pub blurb: &'static CStr,
    /// Required arguments are passed by position, in priority order.
    pub priority: c_int,
    pub flags: sys::VipsArgumentFlags,
}

impl Arg {
    fn new(
        flags: sys::VipsArgumentFlags,
        name: &'static CStr,
        priority: c_int,
        nick: &'static CStr,
        blurb: &'static CStr,
    ) -> Self {
        Arg {
            name,
            nick,
            blurb,
            priority,
            flags,
        }
    }

    pub fn required_input(
        name: &'static CStr,
        priority: c_int,
        nick: &'static CStr,
        blurb: &'static CStr,
    ) -> Self {
        Self::new(
            sys::VIPS_ARGUMENT_REQUIRED_INPUT,
            name,
            priority,
            nick,
            blurb,
        )
    }

    pub fn optional_input(
        name: &'static CStr,
        priority: c_int,
        nick: &'static CStr,
        blurb: &'static CStr,
    ) -> Self {
        Self::new(
            sys::VIPS_ARGUMENT_OPTIONAL_INPUT,
            name,
            priority,
            nick,
            blurb,
        )
    }

    pub fn required_output(
        name: &'static CStr,
        priority: c_int,
        nick: &'static CStr,
        blurb: &'static CStr,
    ) -> Self {
        Self::new(
            sys::VIPS_ARGUMENT_REQUIRED_OUTPUT,
            name,
            priority,
            nick,
            blurb,
        )
    }

    pub fn optional_output(
        name: &'static CStr,
        priority: c_int,
        nick: &'static CStr,
        blurb: &'static CStr,
    ) -> Self {
        Self::new(
            sys::VIPS_ARGUMENT_OPTIONAL_OUTPUT,
            name,
            priority,
            nick,
            blurb,
        )
    }

    pub fn deprecated(mut self) -> Self {
        self.flags |= sys::VIPS_ARGUMENT_DEPRECATED;
        self
    }
}

/// Integer types vips stores enums and flags in.
///
/// # Safety
/// Must be 32-bit integers.
pub unsafe trait EnumStorage {}
// SAFETY: 32 bits
unsafe impl EnumStorage for i32 {}
// SAFETY: 32 bits
unsafe impl EnumStorage for u32 {}

/// The class of `T` during [`ObjectSubclass::class_init`].
pub struct Class<T: ObjectSubclass> {
    raw: *mut T::Class,
    _marker: PhantomData<T>,
}

impl<T: ObjectSubclass> Class<T> {
    /// The class struct, for anything without a wrapper.
    ///
    /// # Safety
    ///
    /// Function pointers stored in the class are called by libvips with
    /// instances of `T`, and must be sound for that.
    pub unsafe fn raw(&mut self) -> &mut T::Class {
        &mut *self.raw
    }

    pub(crate) fn raw_ptr(&mut self) -> *mut T::Class {
        self.raw
    }

    fn install(&mut self, pspec: *mut sys::GParamSpec, arg: &Arg, offset: usize) {
        // SAFETY: the class is a VipsObjectClass being initialised, the
        // pspec is new, and offset points at a field of the right type,
        // see Field
        unsafe {
            sys::g_object_class_install_property(
                self.raw as *mut sys::GObjectClass,
                sys::vips_argument_get_id() as c_uint,
                pspec,
            );
            sys::vips_object_class_install_argument(
                self.raw as *mut sys::VipsObjectClass,
                pspec,
                arg.flags,
                arg.priority,
                offset as c_uint,
            );
        }
    }

    /// `VIPS_ARG_STRING()`. vips owns the string, and frees it.
    pub fn arg_string(
        &mut self,
        arg: Arg,
        field: Field<T, *mut c_char>,
        default: Option<&'static CStr>,
    ) {
        let default = default.map_or(std::ptr::null(), CStr::as_ptr);
        // SAFETY: all strings are static
        let pspec = unsafe {
            sys::g_param_spec_string(
                arg.name.as_ptr(),
                arg.nick.as_ptr(),
                arg.blurb.as_ptr(),
                default,
                sys::G_PARAM_READWRITE,
            )
        };
        self.install(pspec, &arg, field.offset);
    }

    /// `VIPS_ARG_BOOL()`.
    pub fn arg_bool(&mut self, arg: Arg, field: Field<T, sys::gboolean>, default: bool) {
        // SAFETY: all strings are static
        let pspec = unsafe {
            sys::g_param_spec_boolean(
                arg.name.as_ptr(),
                arg.nick.as_ptr(),
                arg.blurb.as_ptr(),
                default as sys::gboolean,
                sys::G_PARAM_READWRITE,
            )
        };
        self.install(pspec, &arg, field.offset);
    }

    /// `VIPS_ARG_INT()`.
    pub fn arg_int(
        &mut self,
        arg: Arg,
        field: Field<T, c_int>,
        min: c_int,
        max: c_int,
        default: c_int,
    ) {
        assert!(min <= default && default <= max);
        // SAFETY: all strings are static
        let pspec = unsafe {
            sys::g_param_spec_int(
                arg.name.as_ptr(),
                arg.nick.as_ptr(),
                arg.blurb.as_ptr(),
                min,
                max,
                default,
                sys::G_PARAM_READWRITE,
            )
        };
        self.install(pspec, &arg, field.offset);
    }

    /// `VIPS_ARG_DOUBLE()`.
    pub fn arg_double(&mut self, arg: Arg, field: Field<T, f64>, min: f64, max: f64, default: f64) {
        assert!(min <= default && default <= max);
        // SAFETY: all strings are static
        let pspec = unsafe {
            sys::g_param_spec_double(
                arg.name.as_ptr(),
                arg.nick.as_ptr(),
                arg.blurb.as_ptr(),
                min,
                max,
                default,
                sys::G_PARAM_READWRITE,
            )
        };
        self.install(pspec, &arg, field.offset);
    }

    /// `VIPS_ARG_ENUM()`, `enum_type` is eg. `vips_access_get_type()`.
    pub fn arg_enum<E: EnumStorage>(
        &mut self,
        arg: Arg,
        field: Field<T, E>,
        enum_type: sys::GType,
        default: c_int,
    ) {
        // SAFETY: all strings are static, g_param_spec_enum() checks the
        // type and the default
        let pspec = unsafe {
            sys::g_param_spec_enum(
                arg.name.as_ptr(),
                arg.nick.as_ptr(),
                arg.blurb.as_ptr(),
                enum_type,
                default,
                sys::G_PARAM_READWRITE,
            )
        };
        self.install(pspec, &arg, field.offset);
    }

    /// `VIPS_ARG_IMAGE()`.
    pub fn arg_image(&mut self, arg: Arg, field: Field<T, *mut sys::VipsImage>) {
        // SAFETY: all strings are static
        let pspec = unsafe {
            sys::g_param_spec_object(
                arg.name.as_ptr(),
                arg.nick.as_ptr(),
                arg.blurb.as_ptr(),
                sys::vips_image_get_type(),
                sys::G_PARAM_READWRITE,
            )
        };
        self.install(pspec, &arg, field.offset);
    }

    /// `VIPS_ARG_BOXED()`, `boxed_type` is eg. `vips_array_double_get_type()`.
    pub fn arg_boxed<B>(&mut self, arg: Arg, field: Field<T, *mut B>, boxed_type: sys::GType) {
        // SAFETY: all strings are static
        let pspec = unsafe {
            sys::g_param_spec_boxed(
                arg.name.as_ptr(),
                arg.nick.as_ptr(),
                arg.blurb.as_ptr(),
                boxed_type,
                sys::G_PARAM_READWRITE,
            )
        };
        self.install(pspec, &arg, field.offset);
    }
}

impl<T: ObjectSubclass> Class<T>
where
    T::Class: IsOperationClass,
{
    /// Add to the class `VipsOperationFlags`, eg. `VIPS_OPERATION_SEQUENTIAL`.
    pub fn add_operation_flags(&mut self, flags: sys::VipsOperationFlags) {
        let operation_class = self.raw as *mut sys::VipsOperationClass;
        // SAFETY: the class starts with a VipsOperationClass
        unsafe { (*operation_class).flags |= flags };
    }
}

/// Operations with a `build` vfunc, see [`Class::install_build`].
pub trait Build: ObjectSubclass + StaticType {
    /// Called after the parent's `build`, which checks that all required
    /// arguments are set.
    fn build(&mut self) -> Result<()>;
}

impl<T: Build> Class<T> {
    pub fn install_build(&mut self) {
        let object_class = self.raw as *mut sys::VipsObjectClass;
        // SAFETY: the trampoline is called with instances of T
        unsafe { (*object_class).build = Some(build_trampoline::<T>) };
    }
}

unsafe extern "C" fn build_trampoline<T: Build>(object: *mut sys::VipsObject) -> c_int {
    let parent = parent_class::<T>() as *mut sys::VipsObjectClass;
    if let Some(build) = (*parent).build {
        if build(object) != 0 {
            return -1;
        }
    }

    error::guard(T::NICKNAME, || T::build(&mut *(object as *mut T)))
}

/// Set output argument `name` to `image`, like
/// `g_object_set(object, name, image, NULL)` in C, and return a new
/// reference to it.
pub fn set_output_image<T: ObjectSubclass>(
    object: &mut T,
    name: &CStr,
    image: Image,
) -> Result<Image> {
    let gobject = object as *mut T as *mut sys::GObject;

    // SAFETY: T is a VipsObject, name is a C string, and the out pointers
    // are valid
    let flags = unsafe {
        let mut pspec = std::ptr::null_mut();
        let mut argument_class = std::ptr::null_mut();
        let mut argument_instance = std::ptr::null_mut();
        error::check(sys::vips_object_get_argument(
            gobject as *mut sys::VipsObject,
            name.as_ptr(),
            &mut pspec,
            &mut argument_class,
            &mut argument_instance,
        ))?;
        (*argument_class).flags
    };
    if flags & sys::VIPS_ARGUMENT_OUTPUT == 0 {
        return Err(Error::new(format!(
            "{} is not an output argument",
            name.to_string_lossy()
        )));
    }

    let result = image.clone();

    // SAFETY: GValue is initialised before use and unset after; the
    // property is an object property, so a VipsImage fits
    unsafe {
        let mut value: sys::GValue = std::mem::zeroed();
        sys::g_value_init(&mut value, sys::vips_image_get_type());
        sys::g_value_set_object(&mut value, image.as_ptr() as *mut c_void);
        sys::g_object_set_property(gobject, name.as_ptr(), &value);
        sys::g_value_unset(&mut value);
    }

    // vips does not ref output arguments: the operation now owns the
    // reference we were given
    std::mem::forget(image);

    Ok(result)
}
