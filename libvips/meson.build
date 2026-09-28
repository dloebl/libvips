libvips_sources = []
libvips_components = []
subdir('include/vips')
if rust_enabled
    subdir('rust')
endif
subdir('foreign')
if get_option('deprecated')
    subdir('deprecated')
endif
subdir('arithmetic')
subdir('resample')
subdir('colour')
subdir('conversion')
subdir('convolution')
subdir('freqfilt')
subdir('histogram')
subdir('draw')
subdir('iofuncs')
subdir('morphology')
subdir('mosaicing')
subdir('create')

# All Rust code goes into one C-ABI static library, so there is a single
# copy of the Rust standard library. Its symbols are hidden in the shared
# libvips: the exports stay exactly the VIPS_API functions of the C headers.
libvips_link_with = []
libvips_link_args = nodelete_link_args
libvips_link_depends = []
if rust_enabled
    vips_rust_staticlib = static_library('vips_rust',
        'rust/vips_rust.rs',
        rust_abi: 'c',
        # LTO drops most of the Rust standard library
        rust_args: ['-Clto=true', '-Cembed-bitcode=yes', '-Ccodegen-units=1'],
        link_with: libvips_rust_crates,
        override_options: ['rust_std=2021'],
        # a static libvips needs it at link time
        install: get_option('default_library') != 'shared',
    )

    if get_option('default_library') == 'shared' and host_os == 'darwin'
        libvips_link_args += '-Wl,-load_hidden,' + vips_rust_staticlib.full_path()
        libvips_link_depends += vips_rust_staticlib
    else
        libvips_link_with += vips_rust_staticlib
        if cc.get_linker_id() not in ['link', 'lld-link', 'ld64']
            libvips_link_args += '-Wl,--exclude-libs,libvips_rust.a'
        endif
    endif
endif

libvips_lib = library('vips',
    enumtypes,
    link_whole: libvips_components,
    link_with: libvips_link_with,
    link_depends: libvips_link_depends,
    dependencies: libvips_deps,
    version: library_version,
    darwin_versions: darwin_versions,
    gnu_symbol_visibility: 'hidden',
    install: true,
    link_args: libvips_link_args,
)

# Rust unit tests, run by "meson test". They link their own copy of the
# crates, which clashes with the one in a static libvips.
if rust_enabled and get_option('default_library') != 'static'
    rust_test_deps = [glib_dep, gobject_dep]
    rust_mod.test('rust-vips', vips_rust_lib,
        link_with: libvips_lib, dependencies: rust_test_deps)
    rust_mod.test('rust-foreign', vips_foreign_rust_lib,
        link_with: libvips_lib, dependencies: rust_test_deps)
endif

libvips_dep = declare_dependency(
    link_with: libvips_lib,
    dependencies: libvips_deps,
)

pkg.generate(
    libvips_lib,
    libraries: [ glib_dep, gio_dep, gobject_dep ],
    name: 'vips',
    description: 'Image processing library',
)

if enable_introspection
    vips_gir = gnome.generate_gir(
        libvips_lib,
        namespace: 'Vips',
        nsversion: '@0@.0'.format(version_major),
        identifier_prefix: 'Vips',
        symbol_prefix: 'vips',
        export_packages: 'vips',
        header: 'vips/vips.h',
        sources: libvips_sources,
        dependencies: libvips_deps,
        includes: ['GObject-2.0', 'Gio-2.0'],
        install: true
    )

    if get_option('vapi')
        gnome.generate_vapi(
            'vips',
            sources: vips_gir[0],
            packages: [ 'glib-2.0', 'gio-2.0', 'gobject-2.0' ],
            install: true
        )
    endif
endif

#
# The following configuration is only valid when the modules are enabled
#
if not modules_enabled
    subdir_done()
endif

if magick_module
    shared_module('vips-magick',
        'module/magick.c',
        magick_module_sources,
        magick_module_headers,
        name_prefix: '',
        dependencies: [libvips_dep, magick_dep],
        install: true,
        install_dir: module_dir
    )
endif

if libjxl_module
    shared_module('vips-jxl',
        'module/jxl.c',
        jpeg_xl_module_sources,
        name_prefix: '',
        dependencies: [libvips_dep, libjxl_dep, libjxl_threads_dep],
        install: true,
        install_dir: module_dir
    )
endif

if libheif_module
    shared_module('vips-heif',
        'module/heif.c',
        heif_module_sources,
        name_prefix: '',
        dependencies: [libvips_dep, libheif_dep],
        install: true,
        install_dir: module_dir
    )
endif

if libpoppler_module
    shared_module('vips-poppler',
        'module/poppler.c',
        poppler_module_sources,
        name_prefix: '',
        dependencies: [libvips_dep, libpoppler_dep, cairo_dep],
        install: true,
        install_dir: module_dir
    )
endif

if openslide_module
    shared_module('vips-openslide',
        'module/openslide.c',
        openslide_module_sources,
        name_prefix: '',
        dependencies: [libvips_dep, openslide_dep],
        install: true,
        install_dir: module_dir
    )
endif
