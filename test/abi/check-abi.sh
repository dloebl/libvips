#!/usr/bin/env bash
#
# Check that libvips in the working tree is binary compatible with a base
# git ref. Linux only (abidiff needs ELF and DWARF).
#
# usage: test/abi/check-abi.sh all <base-ref>
#    or: test/abi/check-abi.sh <command> [args]
#
# commands:
#   prepare <base-ref>  export <base-ref> into $ABI_WORKDIR/base/src
#   build base|head     build and install into $ABI_WORKDIR/{base,head}/prefix
#   symbols             abidiff libvips and libvips-cpp (exported symbols and
#                       the types in the public headers); new exported symbols
#                       must be in the vips namespace
#   introspection       diff operations, arguments and enums as seen by
#                       bindings at runtime, see vips-introspect.c
#   runtime             run the base test suite with pyvips compiled against
#                       base, but loading the head libvips
#
# environment:
#   ABI_WORKDIR         scratch directory, default build-abi
#   ABI_MESON_ARGS      extra meson setup args, used for both builds
#   ABI_PYTEST_ARGS     extra pytest args for the runtime check
#
# Accepted differences go in libvips.abignore (abidiff suppressions),
# introspection.ignore (regexps matching introspection lines) and
# runtime.deselect (base tests that may fail on head).

set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
top=$(cd "$here/../.." && pwd)
work=${ABI_WORKDIR:-$top/build-abi}
reports=$work/reports

die() {
	echo "check-abi: $*" >&2
	exit 2
}

need() {
	for tool in "$@"; do
		command -v "$tool" >/dev/null || die "$tool not found"
	done
}

prefix_of() {
	echo "$work/$1/prefix"
}

prepare() {
	local ref=${1:?prepare needs a base ref}
	local sha

	sha=$(git -C "$top" rev-parse --verify "$ref^{commit}")
	rm -rf "$work/base/src"
	mkdir -p "$work/base/src"
	git -C "$top" archive "$sha" | tar -x -C "$work/base/src"
	echo "$sha" >"$work/base/ref"
	echo "base is $ref ($sha)"
}

build() {
	local name=${1:?build needs base or head}
	local src builddir prefix

	case $name in
	base) src=$work/base/src ;;
	head) src=$top ;;
	*) die "build: expected base or head, not $name" ;;
	esac
	[ -f "$src/meson.build" ] || die "no source in $src, run prepare first"

	builddir=$work/$name/build
	prefix=$(prefix_of "$name")
	rm -rf "$prefix"

	local reconfigure=
	[ -f "$builddir/build.ninja" ] && reconfigure=--reconfigure

	# Same options for both builds. A release build like distros ship, plus
	# the debug info abidiff needs (-Ddebug=true would enable leak
	# reporting). deprecated=true gives the largest ABI surface.
	# shellcheck disable=SC2086
	meson setup "$builddir" "$src" $reconfigure \
		--prefix="$prefix" \
		--libdir=lib \
		--buildtype=release \
		-Dc_args=-g \
		-Dcpp_args=-g \
		-Ddeprecated=true \
		-Dmagick=disabled \
		-Dintrospection=disabled \
		-Ddocs=false \
		-Dexamples=false \
		${ABI_MESON_ARGS:-}
	meson compile -C "$builddir"
	meson install -C "$builddir" --quiet
}

exported_symbols() {
	nm -D --defined-only "$1" | awk '{ print $NF }' | LC_ALL=C sort -u
}

# abidiff two libraries, restricted to the types in the public headers.
# Added functions and variables are fine. --leaf-changes-only keeps the
# report readable, and avoids an assertion failure in the default reporter
# of libabigail 2.9.
run_abidiff() {
	local old=$1 new=$2
	shift 2

	abidiff \
		--leaf-changes-only \
		--no-added-syms \
		--headers-dir1 "$(prefix_of base)/include/vips" \
		--headers-dir2 "$(prefix_of head)/include/vips" \
		--drop-private-types \
		--suppressions "$here/libvips.abignore" \
		"$@" "$old" "$new"
}

show_report() {
	head -n 200 "$1"
	[ "$(wc -l <"$1")" -le 200 ] || echo "... see $1 for the full report"
}

symbols() {
	need abidiff nm c++filt
	mkdir -p "$reports"

	local base head failed=0

	base=$(prefix_of base)
	head=$(prefix_of head)

	for lib in libvips libvips-cpp; do
		local old=$base/lib/$lib.so new=$head/lib/$lib.so
		local report=$reports/abidiff-$lib.txt
		local unreachable=$reports/abidiff-$lib-unreachable.txt
		local status=0 unreachable_status=0 counts

		[ -f "$old" ] && [ -f "$new" ] || die "missing $lib, run build first"

		# Exported functions and variables, and all types they use.
		run_abidiff "$old" "$new" >"$report" 2>&1 || status=$?
		if [ "$status" -ne 0 ]; then
			echo "FAIL $lib: abidiff exit status $status" \
				"(1: error, 4: ABI change, 8: incompatible ABI change)"
			show_report "$report"
			failed=1
		else
			echo "PASS $lib: no ABI changes"
		fi

		# Class structs are rarely reachable from exported functions,
		# so check all types in the headers too. abidiff can't be told
		# that adding a type is fine, so read the summary.
		run_abidiff "$old" "$new" --non-reachable-types \
			>"$unreachable" 2>&1 || unreachable_status=$?
		counts=$(sed -nE 's/^Unreachable types summary: ([0-9]+) removed.*, ([0-9]+) changed.*/\1 \2/p' \
			"$unreachable")
		if [ $((unreachable_status & 3)) -ne 0 ] ||
			{ [ "$unreachable_status" -ne 0 ] && [ "$counts" != "0 0" ]; }; then
			echo "FAIL $lib: types changed or removed" \
				"(abidiff exit status $unreachable_status)"
			show_report "$unreachable"
			failed=1
		else
			echo "PASS $lib: no type changes"
		fi

		# New exported symbols must be part of the vips API, not eg.
		# the runtime of a statically linked library.
		exported_symbols "$old" >"$reports/symbols-$lib-base.txt"
		exported_symbols "$new" >"$reports/symbols-$lib-head.txt"
		LC_ALL=C comm -13 "$reports/symbols-$lib-base.txt" \
			"$reports/symbols-$lib-head.txt" >"$reports/added-$lib.txt"

		local strays=$reports/strays-$lib.txt
		if [ "$lib" = libvips ]; then
			grep -Ev '^(vips_|vips__|im_|Vips|VIPS_)' \
				"$reports/added-$lib.txt" >"$strays" || true
		else
			c++filt <"$reports/added-$lib.txt" |
				grep -Fv 'vips::' >"$strays" || true
		fi

		if [ -s "$strays" ]; then
			echo "FAIL $lib: new exported symbols outside the vips namespace:"
			sed 's/^/  /' "$strays"
			failed=1
		fi

		if [ -s "$reports/added-$lib.txt" ]; then
			echo "INFO $lib: $(wc -l <"$reports/added-$lib.txt") new exported symbols:"
			c++filt <"$reports/added-$lib.txt" | sed 's/^/  /'
		fi
	done

	return $failed
}

# Compile the dumper against the headers of $1.
introspect_compile() {
	local prefix
	prefix=$(prefix_of "$1")

	# shellcheck disable=SC2046
	${CC:-cc} -O1 -Wall -o "$work/$1/vips-introspect" \
		"$here/vips-introspect.c" \
		$(PKG_CONFIG_PATH=$prefix/lib/pkgconfig pkg-config --cflags --libs vips) \
		-ldl
}

# Run the dumper compiled against $1 with the libvips of $2.
introspect_run() {
	local libdir get_types

	libdir=$(prefix_of "$2")/lib
	get_types=$(grep -o 'vips_[a-z0-9_]*_get_type' \
		"$(prefix_of "$1")/include/vips/enumtypes.h" | LC_ALL=C sort -u)

	# shellcheck disable=SC2086
	env -u VIPS_BLOCK_UNTRUSTED \
		LD_LIBRARY_PATH="$libdir" DYLD_LIBRARY_PATH="$libdir" \
		VIPS_WARNING=0 \
		"$work/$1/vips-introspect" $get_types | LC_ALL=C sort
}

# Lines of $1 not matched by introspection.ignore.
without_ignored() {
	grep -Ev -f <(grep -Ev '^[[:space:]]*(#|$)' "$here/introspection.ignore") \
		"$1" || true
}

introspection() {
	need pkg-config
	mkdir -p "$reports"

	local failed=0

	introspect_compile base
	introspect_compile head

	local old=$reports/introspection-base.txt new=$reports/introspection-head.txt
	local cross=$reports/introspection-cross.txt cross_status=0

	introspect_run base base >"$old" || die "vips-introspect failed on base"
	introspect_run head head >"$new" || die "vips-introspect failed on head"
	# a binary built against base, running on head
	introspect_run base head >"$cross" || cross_status=$?

	[ -s "$old" ] && [ -s "$new" ] || die "introspection produced no output"

	LC_ALL=C comm -23 "$old" "$new" >"$reports/introspection-removed.txt"
	LC_ALL=C comm -13 "$old" "$new" >"$reports/introspection-added.txt"
	without_ignored "$reports/introspection-removed.txt" \
		>"$reports/introspection-breaks.txt"

	if [ -s "$reports/introspection-breaks.txt" ]; then
		echo "FAIL introspection: removed or changed in head:"
		sed 's/^/  - /' "$reports/introspection-breaks.txt"
		echo "added or changed in head:"
		sed 's/^/  + /' "$reports/introspection-added.txt"
		failed=1
	else
		echo "PASS introspection: nothing removed or changed" \
			"($(wc -l <"$new") facts)"
		if [ -s "$reports/introspection-added.txt" ]; then
			echo "INFO introspection: added in head:"
			sed 's/^/  + /' "$reports/introspection-added.txt"
		fi
	fi

	# The base binary reads head's class structs with base's layout, an
	# end-to-end check of what abidiff reports for the headers.
	if [ "$cross_status" -ne 0 ]; then
		echo "FAIL introspection: a binary built against base fails on" \
			"head libvips (exit status $cross_status)"
		failed=1
	elif ! cmp -s "$cross" "$new"; then
		echo "FAIL introspection: a binary built against base sees a" \
			"different head libvips than one built against head:"
		diff -U0 "$new" "$cross" || true
		failed=1
	else
		echo "PASS introspection: base binary on head libvips agrees"
	fi

	return $failed
}

runtime() {
	need python3 pkg-config

	local base head venv
	base=$(prefix_of base)
	head=$(prefix_of head)
	venv=$work/venv

	[ -d "$work/base/src/test/test-suite" ] || die "no base source, run prepare first"

	# pyvips in API mode is a C extension, compile it against base.
	rm -rf "$venv"
	python3 -m venv "$venv"
	PKG_CONFIG_PATH=$base/lib/pkgconfig \
		"$venv/bin/pip" install --quiet --no-cache-dir \
		--no-binary pyvips 'pyvips[test]'

	export LD_LIBRARY_PATH=$head/lib DYLD_LIBRARY_PATH=$head/lib
	ABI_HEAD_LIBDIR=$head/lib \
		ABI_BASE_VERSION=$(PKG_CONFIG_PATH=$base/lib/pkgconfig pkg-config --modversion vips) \
		"$venv/bin/python" - <<-'EOF'
		import os, sys
		import pyvips

		# pyvips only uses its compiled module with the libvips minor
		# version it was compiled against, and falls back to ABI mode
		base = os.environ["ABI_BASE_VERSION"].split(".")[:2]
		head = [str(pyvips.version(0)), str(pyvips.version(1))]
		if not pyvips.API_mode:
		    if base == head:
		        sys.exit("pyvips is in ABI mode, it must be compiled against base")
		    print(f"pyvips is in ABI mode, base {'.'.join(base)} and "
		          f"head {'.'.join(head)} differ in minor version")

		# make sure we really run on the head libvips
		if os.path.exists("/proc/self/maps"):
		    libdir = os.environ["ABI_HEAD_LIBDIR"]
		    loaded = {line.split()[-1] for line in open("/proc/self/maps")
		              if "libvips.so" in line}
		    stray = [p for p in loaded if not p.startswith(libdir + "/")]
		    if not loaded or stray:
		        sys.exit(f"expected libvips from {libdir}, got {loaded}")

		mode = "API" if pyvips.API_mode else "ABI"
		print(f"pyvips {pyvips.__version__} ({mode} mode) on libvips "
		      f"{pyvips.version(0)}.{pyvips.version(1)}.{pyvips.version(2)}")
	EOF

	# The base expectations must still hold on head, except for the
	# accepted behaviour changes in runtime.deselect.
	local deselect=()
	while read -r id; do
		deselect+=(--deselect "$id")
	done < <(grep -Ev '^[[:space:]]*(#|$)' "$here/runtime.deselect")

	cd "$work/base/src/test/test-suite"
	# shellcheck disable=SC2086
	"$venv/bin/python" -m pytest -q -p no:cacheprovider \
		${deselect[@]+"${deselect[@]}"} ${ABI_PYTEST_ARGS:-} .
}

all() {
	local ref=${1:?all needs a base ref} failed=0

	prepare "$ref"
	build base
	build head

	# separate processes, so errexit still applies inside each check
	"$0" symbols || failed=1
	"$0" introspection || failed=1
	"$0" runtime || failed=1

	[ "$failed" -eq 0 ] && echo "ABI check passed" || echo "ABI check FAILED"
	return $failed
}

case ${1:-} in
prepare | build | symbols | introspection | runtime | all)
	"$@"
	;;
*)
	sed -n '3,/^$/s/^# \{0,1\}//p' "$0" >&2
	exit 2
	;;
esac
