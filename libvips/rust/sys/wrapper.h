/* Input for bindgen, see meson.build.
 *
 * The vips7 compatibility macros are not needed from Rust, and leaving them
 * out keeps the bindings the same with and without -Ddeprecated.
 */
#define VIPS_DISABLE_COMPAT
#include <vips/vips.h>
