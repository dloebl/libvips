# The crates every in-tree Rust operation builds on, see README.md:
#
#   vips_sys  raw bindings, generated from the headers with bindgen
#   vips      the safe layer on top
#
# Both are rlibs. Operation crates (eg. libvips/foreign/lib.rs) add
# themselves to libvips_rust_crates, and libvips/meson.build links them all
# into a single C-ABI static library.
rust_mod = import('rust')

libvips_rust_crates = []

# Every rlib is compiled with bitcode, so the final static library can be
# built with LTO.
rust_rlib_args = ['-Cembed-bitcode=yes']

vips_sys_bindings = rust_mod.bindgen(
    input: 'sys/wrapper.h',
    output: 'bindings.rs',
    include_directories: libvips_includedir,
    # libvips_headers_dep also orders us after the generated
    # vips/version.h and vips/enumtypes.h
    dependencies: [libvips_headers_dep, glib_dep, gio_dep, gobject_dep],
    args: [
        '--allowlist-function', '(vips|g_object|g_type|g_param_spec|g_value)_.*|g_free',
        '--allowlist-type', '_?(Vips|GObject|GTypeInfo|GParamSpec|GValue).*',
        '--allowlist-var', 'VIPS_.*',
        '--default-enum-style', 'consts',
        '--no-prepend-enum-name',
        '--no-doc-comments',
    ],
)

vips_sys_lib = static_library('vips_sys',
    structured_sources(['sys/lib.rs', vips_sys_bindings]),
    rust_abi: 'rust',
    rust_args: rust_rlib_args,
    override_options: ['rust_std=2021'],
)

vips_rust_lib = static_library('vips',
    'vips/lib.rs',
    rust_abi: 'rust',
    rust_args: rust_rlib_args,
    link_with: [vips_sys_lib],
    override_options: ['rust_std=2021'],
)
