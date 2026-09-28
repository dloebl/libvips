/* Dump the runtime contract of libvips, one fact per line.
 *
 * Language bindings (pyvips, ruby-vips, NetVips, ...) look up operations
 * by nickname, pass arguments by name and enums by nick, all at runtime. That
 * contract is as much part of libvips binary compatibility as the exported
 * symbols, but abidiff can't see it. check-abi.sh diffs the output of this
 * program between two builds.
 *
 * Every line is self-contained, so the sorted output can be compared as a
 * set: lines only in the new build are compatible additions, lines missing
 * from the new build are removals or changes.
 *
 * Human-readable text (descriptions, blurbs) is left out on purpose, and
 * deprecation is reported on separate lines, so that deprecating something
 * shows up as an addition.
 *
 * Only public API is used, so this builds against any libvips >= 8.17.
 *
 * usage: vips-introspect [get-type-function ...]
 *
 * Functions named on the command line (eg. vips_band_format_get_type) are
 * looked up with dlsym() and called, to register types that no argument
 * references.
 */

#ifndef _GNU_SOURCE
#define _GNU_SOURCE /* for RTLD_DEFAULT */
#endif

#include <stdio.h>
#include <string.h>
#include <dlfcn.h>

#include <vips/vips.h>

static void
append_double(GString *out, double d)
{
	char buf[G_ASCII_DTOSTR_BUF_SIZE];

	/* The shortest representation that round-trips.
	 */
	g_ascii_formatd(buf, sizeof(buf), "%.15g", d);
	if (g_ascii_strtod(buf, NULL) != d)
		g_ascii_formatd(buf, sizeof(buf), "%.17g", d);

	g_string_append(out, buf);
}

static void
append_enum(GString *out, GType type, int value)
{
	GEnumClass *class = g_type_class_ref(type);
	GEnumValue *v = g_enum_get_value(class, value);

	if (v)
		g_string_append(out, v->value_nick);
	else
		g_string_append_printf(out, "%d", value);

	g_type_class_unref(class);
}

/* Flags as a list of single-bit nicks, so aliases and combined values (eg.
 * VIPS_FOREIGN_KEEP_ALL) can't make the output depend on declaration order.
 */
static void
append_flags(GString *out, GType type, guint value)
{
	GFlagsClass *class = g_type_class_ref(type);

	if (value == 0) {
		GFlagsValue *v = g_flags_get_first_value(class, 0);

		g_string_append(out, v ? v->value_nick : "0");
	}

	for (int bit = 0; bit < 32; bit++) {
		guint mask = 1u << bit;
		GFlagsValue *v;

		if (!(value & mask))
			continue;

		if (value & (mask - 1))
			g_string_append_c(out, '|');

		if ((v = g_flags_get_first_value(class, mask)))
			g_string_append(out, v->value_nick);
		else
			g_string_append_printf(out, "0x%x", mask);
	}

	g_type_class_unref(class);
}

static void
append_pspec(GString *out, GParamSpec *pspec)
{
	GType value_type = G_PARAM_SPEC_VALUE_TYPE(pspec);

	g_string_append_printf(out, " type=%s pspec=%s",
		g_type_name(value_type), G_PARAM_SPEC_TYPE_NAME(pspec));

#define RANGE(TYPE, FMT) \
	{ \
		TYPE *p = (TYPE *) pspec; \
		g_string_append_printf(out, " min=" FMT " max=" FMT " default=" FMT, \
			p->minimum, p->maximum, p->default_value); \
	}

	if (G_IS_PARAM_SPEC_BOOLEAN(pspec))
		g_string_append_printf(out, " default=%s",
			G_PARAM_SPEC_BOOLEAN(pspec)->default_value ? "true" : "false");
	else if (G_IS_PARAM_SPEC_INT(pspec))
		RANGE(GParamSpecInt, "%d")
	else if (G_IS_PARAM_SPEC_UINT(pspec))
		RANGE(GParamSpecUInt, "%u")
	else if (G_IS_PARAM_SPEC_INT64(pspec))
		RANGE(GParamSpecInt64, "%" G_GINT64_FORMAT)
	else if (G_IS_PARAM_SPEC_UINT64(pspec))
		RANGE(GParamSpecUInt64, "%" G_GUINT64_FORMAT)
	else if (G_IS_PARAM_SPEC_DOUBLE(pspec)) {
		GParamSpecDouble *p = G_PARAM_SPEC_DOUBLE(pspec);

		g_string_append(out, " min=");
		append_double(out, p->minimum);
		g_string_append(out, " max=");
		append_double(out, p->maximum);
		g_string_append(out, " default=");
		append_double(out, p->default_value);
	}
	else if (G_IS_PARAM_SPEC_ENUM(pspec)) {
		g_string_append(out, " default=");
		append_enum(out, value_type, G_PARAM_SPEC_ENUM(pspec)->default_value);
	}
	else if (G_IS_PARAM_SPEC_FLAGS(pspec)) {
		g_string_append(out, " default=");
		append_flags(out, value_type, G_PARAM_SPEC_FLAGS(pspec)->default_value);
	}
	else if (G_IS_PARAM_SPEC_STRING(pspec)) {
		const char *def = G_PARAM_SPEC_STRING(pspec)->default_value;

		if (def) {
			char *escaped = g_strescape(def, NULL);

			g_string_append_printf(out, " default=\"%s\"", escaped);
			g_free(escaped);
		}
		else
			g_string_append(out, " default=NULL");
	}

#undef RANGE
}

static void
emit(GString *line)
{
	puts(line->str);
	g_string_truncate(line, 0);
}

static void *
dump_argument(VipsObjectClass *class, GParamSpec *pspec,
	VipsArgumentClass *argument_class, void *a, void *b)
{
	const char *type_name = G_OBJECT_CLASS_NAME(class);
	VipsArgumentFlags flags = argument_class->flags;
	GString *line = (GString *) a;

	g_string_append_printf(line, "arg %s %s", type_name, pspec->name);
	append_pspec(line, pspec);
	g_string_append(line, " flags=");
	append_flags(line, VIPS_TYPE_ARGUMENT_FLAGS,
		flags & ~VIPS_ARGUMENT_DEPRECATED);
	/* Bindings pass required arguments by position, in priority order,
	 * and optional ones by name, so only required priorities matter.
	 */
	if (flags & VIPS_ARGUMENT_REQUIRED)
		g_string_append_printf(line, " priority=%d",
			argument_class->priority);
	emit(line);

	if (flags & VIPS_ARGUMENT_DEPRECATED) {
		g_string_append_printf(line, "arg-deprecated %s %s",
			type_name, pspec->name);
		emit(line);
	}

	return NULL;
}

static void
dump_foreign(GType type, GString *line)
{
	const char *type_name = g_type_name(type);
	VipsForeignClass *foreign_class = g_type_class_peek(type);

	g_string_append_printf(line, "foreign %s priority=%d suffs=",
		type_name, foreign_class->priority);
	if (foreign_class->suffs)
		for (const char **p = foreign_class->suffs; *p; p++)
			g_string_append_printf(line, "%s%s",
				p == foreign_class->suffs ? "" : ",", *p);
	emit(line);

	if (g_type_is_a(type, VIPS_TYPE_FOREIGN_LOAD)) {
		VipsForeignLoadClass *load_class = g_type_class_peek(type);

		/* Which is_a flavours exist decides which of
		 * vips_foreign_find_load*() can find this loader.
		 */
		g_string_append_printf(line,
			"load %s is_a=%d is_a_buffer=%d is_a_source=%d",
			type_name,
			load_class->is_a != NULL,
			load_class->is_a_buffer != NULL,
			load_class->is_a_source != NULL);
		emit(line);
	}

	if (g_type_is_a(type, VIPS_TYPE_FOREIGN_SAVE)) {
		VipsForeignSaveClass *save_class = g_type_class_peek(type);

		g_string_append_printf(line, "save %s saveable=", type_name);
		append_flags(line, VIPS_TYPE_FOREIGN_SAVEABLE, save_class->saveable);
		g_string_append(line, " coding=");
		append_flags(line, VIPS_TYPE_FOREIGN_CODING, save_class->coding);
		g_string_append(line, " format_table=");
		if (save_class->format_table)
			for (int i = 0; i < VIPS_FORMAT_LAST; i++) {
				if (i > 0)
					g_string_append_c(line, ',');
				append_enum(line, VIPS_TYPE_BAND_FORMAT, i);
				g_string_append_c(line, ':');
				append_enum(line, VIPS_TYPE_BAND_FORMAT,
					save_class->format_table[i]);
			}
		else
			g_string_append(line, "NULL");
		emit(line);
	}
}

static void *
dump_type(GType type, void *a)
{
	const char *type_name = g_type_name(type);
	gboolean abstract = G_TYPE_IS_ABSTRACT(type);
	VipsObjectClass *class = VIPS_OBJECT_CLASS(g_type_class_ref(type));
	GString *line = (GString *) a;

	g_string_append_printf(line, "type %s parent=%s abstract=%d nickname=%s",
		type_name,
		g_type_name(g_type_parent(type)),
		abstract,
		class->nickname ? class->nickname : "NULL");
	emit(line);

	if (class->deprecated) {
		g_string_append_printf(line, "type-deprecated %s", type_name);
		emit(line);
	}

	/* One line per flag, so that adding one (eg. deprecated or untrusted)
	 * is an addition, and removing one is not.
	 */
	if (g_type_is_a(type, VIPS_TYPE_OPERATION)) {
		VipsOperationFlags flags = VIPS_OPERATION_CLASS(class)->flags;

		for (int bit = 0; bit < 32; bit++)
			if (flags & (1u << bit)) {
				g_string_append_printf(line, "operation-flag %s ",
					type_name);
				append_flags(line, VIPS_TYPE_OPERATION_FLAGS, 1u << bit);
				emit(line);
			}
	}

	if (g_type_is_a(type, VIPS_TYPE_FOREIGN))
		dump_foreign(type, line);

	/* Abstract classes can't be instantiated, so their arguments are only
	 * visible through their subclasses, which list them all.
	 */
	if (!abstract)
		vips_argument_class_map(class, dump_argument, line, NULL);

	/* Never unref, classes are not unloaded.
	 */
	return NULL;
}

static void
dump_enum_values(GType type, GString *line)
{
	const char *type_name = g_type_name(type);

	if (G_TYPE_IS_ENUM(type)) {
		GEnumClass *class = g_type_class_ref(type);

		for (guint i = 0; i < class->n_values; i++) {
			GEnumValue *v = &class->values[i];

			g_string_append_printf(line, "enum %s %d %s %s",
				type_name, v->value, v->value_name, v->value_nick);
			emit(line);
		}
		g_type_class_unref(class);
	}
	else {
		GFlagsClass *class = g_type_class_ref(type);

		for (guint i = 0; i < class->n_values; i++) {
			GFlagsValue *v = &class->values[i];

			g_string_append_printf(line, "flags %s 0x%x %s %s",
				type_name, v->value, v->value_name, v->value_nick);
			emit(line);
		}
		g_type_class_unref(class);
	}
}

static void
dump_enums(GType fundamental, GString *line)
{
	guint n_children;
	GType *children = g_type_children(fundamental, &n_children);

	for (guint i = 0; i < n_children; i++)
		if (g_str_has_prefix(g_type_name(children[i]), "Vips"))
			dump_enum_values(children[i], line);

	g_free(children);
}

int
main(int argc, char **argv)
{
	GString *line;

	if (VIPS_INIT(argv[0]))
		vips_error_exit(NULL);

	for (int i = 1; i < argc; i++) {
		GType (*get_type)(void);

		if (!(*(void **) &get_type = dlsym(RTLD_DEFAULT, argv[i])))
			vips_error_exit("%s: symbol not found", argv[i]);
		(void) get_type();
	}

	line = g_string_new(NULL);

	/* Types first: referencing their classes registers the enum types
	 * the arguments use.
	 */
	vips_type_map_all(VIPS_TYPE_OBJECT, dump_type, line);
	dump_enums(G_TYPE_ENUM, line);
	dump_enums(G_TYPE_FLAGS, line);

	g_string_free(line, TRUE);
	vips_shutdown();

	return 0;
}
