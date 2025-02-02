#include <vips/vips.h>

#define MAX_ARG_LEN 4096 // =VIPS_PATH_MAX

extern "C" int
entry(int argc, char **argv);

extern "C" int
LLVMFuzzerInitialize(int *argc, char ***argv)
{
	if (VIPS_INIT(*argv[0]))
		return -1;

	vips_concurrency_set(1);
	return 0;
}

static char *
ExtractLine(const guint8 *data, size_t size, size_t *n)
{
	const guint8 *end;

	end = static_cast<const guint8 *>(
		memchr(data, '\n', VIPS_MIN(size, MAX_ARG_LEN)));
	if (end == nullptr)
		return nullptr;

	*n = end - data;
	return g_strndup(reinterpret_cast<const char *>(data), *n);
}

extern "C" int
LLVMFuzzerTestOneInput(const guint8 *data, size_t size)
{
	void *buf;
	char *cmdline;
	size_t len, n;
	int argn = 0;
	char** argv = NULL;

	cmdline = ExtractLine(data, size, &n);
	if (cmdline == nullptr)
		return 0;

	data += n + 1;
	size -= n + 1;
	
	g_shell_parse_argv(cmdline, &argn, &argv, NULL);

	// We're done with cmdline, free early.
	g_free(cmdline);

	// call the entry point of the vips tool
	entry(argn, argv);

	g_free(buf);
	g_free(argv);

	return 0;
}
