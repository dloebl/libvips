# Operations in Rust

libvips operations can be written in Rust. They are GObject subclasses just
like the C ones, registered the same way, so libvips, the C and C++ APIs and
language bindings can't tell them apart, and the ABI does not change (see
`test/abi/check-abi.sh`).

## Building

```
meson setup build -Drust=enabled
```

This needs meson >= 1.3, rustc >= 1.77 and bindgen (with libclang). There
are no other dependencies: no cargo, no crates.io. `-Drust` defaults to
`disabled`, and `auto` enables Rust if the tools are found.

## Layout

| crate          | source                        | what                                    |
| -------------- | ----------------------------- | --------------------------------------- |
| `vips_sys`     | `rust/sys/`                   | raw bindings, generated with bindgen    |
| `vips`         | `rust/vips/`                  | the safe layer operations are built on  |
| `vips_foreign` | `foreign/lib.rs`, `*.rs`      | loaders and savers, eg. `bmpload.rs`    |
| `vips_rust`    | `rust/vips_rust.rs`           | the static library linked into libvips  |

The bindings are generated from the real headers at build time, so struct
layouts come from libclang exactly as the C compiler sees them, and bindgen
adds compile-time checks of every size and offset.

All crates are linked into one C-ABI static library, `vips_rust`, so there
is a single copy of the Rust standard library, and LTO drops most of it.
libvips hides all its symbols (`--exclude-libs` for ELF, `-load_hidden` for
Mach-O, and DLLs only export `dllexport` symbols anyway), so the exports of
libvips stay exactly the `VIPS_API` functions of the C headers. Rust code
never defines public API directly: C calls into Rust, eg. to register types
from `vips_foreign_operation_init()`.

## Writing an operation

Operations follow the C ones closely, see `foreign/bmpload.rs` for a loader
and `rust/vips/tests.rs` for an operation with inputs, outputs and a
`build` vfunc. In short:

```rust
#[repr(C)]
pub struct Invert {
    parent: sys::VipsOperation, // parent instance, first
    input: *mut sys::VipsImage, // arguments, set by vips
    output: *mut sys::VipsImage,
}

// SAFETY: repr(C), starts with its parent, valid zeroed, owns nothing
unsafe impl ObjectSubclass for Invert {
    const NAME: &'static CStr = c"VipsInvertRust";
    const NICKNAME: &'static CStr = c"invert_rust";
    const DESCRIPTION: &'static CStr = c"invert an image";
    type Class = sys::VipsOperationClass;

    fn parent_type() -> sys::GType {
        // SAFETY: plain type lookup
        unsafe { sys::vips_operation_get_type() }
    }

    fn class_init(class: &mut Class<Self>) {
        class.install_build();
        class.arg_image(Arg::required_input(c"in", 1, c"Input", c"Input image"),
            field!(Invert, input));
        class.arg_image(Arg::required_output(c"out", 2, c"Output", c"Output image"),
            field!(Invert, output));
    }
}

impl Build for Invert {
    fn build(&mut self) -> Result<()> {
        // SAFETY: a required argument, so set
        let input = unsafe { Image::from_borrowed(self.input) }.ok_or(Error::Vips)?;
        let out = set_output_image(self, c"out", Image::new()?)?;
        out.pipeline(sys::VIPS_DEMAND_STYLE_THINSTRIP, &[&input])?;
        out.generate(InvertGenerator { input })   // see generate.rs
    }
}

// defines the C vips_invert_rust_get_type(), call it from C to register
object_type!(vips_invert_rust_get_type, Invert);
```

The rules the `unsafe impl ObjectSubclass` promises to keep:

- the struct is `#[repr(C)]` and starts with the parent instance struct
- all-zero bytes are a valid value, since GObject zero-fills instances: use
  raw pointers, integers, `Option<Box<_>>` or `Option<Arc<_>>`
- non-zero argument defaults are set in `instance_init()`, vips does not
  apply the defaults given to `arg_*()`
- anything that owns memory is dropped in `finalize()`

Every `unsafe` block gets a `// SAFETY:` comment, clippy checks this.

### Errors and panics

Functions return `vips::Result`. Every callback from C (vfuncs, generate,
`*_get_type()`) goes through `vips::error::guard`, which reports errors to
the vips error buffer under the operation nickname and returns `-1`, like C
code does. Panics are caught there too and reported as
`nickname: internal error: ...`, so a bug fails the operation instead of
aborting the process. Pixel access through `Region::line()` and
`OutRegion::line_mut()` checks bounds and the element type, and panics
rather than touching memory outside the region.

## Testing

```
meson test -C build rust-vips rust-foreign    # Rust unit tests
pytest test/test-suite                        # the usual Python tests
test/abi/check-abi.sh all origin/master       # ABI, see test/abi
```

Lint like CI does:

```
rustfmt --edition 2021 --check libvips/rust/vips/lib.rs libvips/foreign/lib.rs
CLIPPY_CONF_DIR=$PWD/libvips/rust RUSTC=clippy-driver meson setup build-clippy \
    -Drust=enabled "-Drust_args=-Dwarnings -Wclippy::all -Wclippy::undocumented_unsafe_blocks"
ninja -C build-clippy libvips/libvips_rust.a
```

## Sanitizers and fuzzing

Static and shared builds both work, including with `-Db_sanitize` and
libFuzzer (`fuzz/oss_fuzz_build.sh` enables Rust when rustc and bindgen are
installed). Meson does not instrument Rust code: with a nightly rustc (or
`RUSTC_BOOTSTRAP=1`), add `-Drust_args=-Zsanitizer=address
-Cunsafe-allow-abi-mismatch=sanitizer`. MSan and TSan need an instrumented
Rust standard library, so keep Rust disabled for them.

## Platforms and limits

- Tested: Linux (arm64) with GCC and Clang, static and shared, rustc 1.77
  to stable, and macOS arm64.
- Untested: Windows, big-endian, cross builds. Rust code must decode file
  formats with explicit endianness (`u32::from_le_bytes()`); cross builds
  need `bindgen_clang_arguments` with the target in the cross file.
- A static libvips contains the Rust standard library. Linking it into a
  program that also links another Rust static library gives duplicate
  symbols, a general limitation of Rust static libraries.
