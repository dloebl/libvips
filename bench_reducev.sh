#!/usr/bin/env bash
#
# bench_reducev.sh - A/B benchmark for the reducev auto-vectorise / ILP work.
#
# Builds two libvips variants with meson (release, -march=native -O3):
#   base   = where this branch forked from upstream  (no optimisation)
#   branch = current HEAD                             (blocked reducev)
# then times:
#   1. isolated `vips reducev`  (single-thread, isolates the inner loop)
#   2. end-to-end `vips resize` (all threads, real-world pipeline)
# for several pixel formats, checks the outputs are bit-exact, and prints
# base-vs-branch speedup tables.
#
# IMPORTANT: each build is run *in place* from its own build/ dir so the
# relative rpath resolves to the matching libvips.so - copying the binary
# elsewhere would silently load the system libvips and give bogus numbers.
#
# Usage:
#   ./bench_reducev.sh                 # auto-detect base, use HEAD as branch
#   BASE_REF=abc123 ./bench_reducev.sh # pin the base commit explicitly
#   REPS=9 W=12000 H=12000 ./bench_reducev.sh
#   REBUILD=1 ./bench_reducev.sh       # force a fresh build of both variants
#
# Tunables (env vars, with defaults):
#   BASE_REF   auto  commit/ref for the "before" build (default: fork point)
#   BRANCH_REF HEAD  commit/ref for the "after" build
#   W, H       16000 test image dimensions
#   VSHRINK    3.0   vertical shrink factor for the isolated reducev test
#   SCALE      0.2   scale factor for the resize test (5x downscale)
#   KERNEL     lanczos3  resample kernel
#   REPS       5     timed repetitions (best/min is reported)
#   WORK_ROOT  /tmp/vips-reducev-bench   where worktrees + builds live
#   REBUILD    0     set to 1 to rebuild even if a build already exists

set -euo pipefail
trap 'rc=$?; echo "" >&2; echo "ERROR: aborted at line $LINENO (exit $rc). See message above." >&2' ERR

# ---- config --------------------------------------------------------------
BRANCH_REF="${BRANCH_REF:-HEAD}"
W="${W:-16000}"
H="${H:-16000}"
VSHRINK="${VSHRINK:-3.0}"
SCALE="${SCALE:-0.2}"
KERNEL="${KERNEL:-lanczos3}"
REPS="${REPS:-5}"
WORK_ROOT="${WORK_ROOT:-/tmp/vips-reducev-bench}"
REBUILD="${REBUILD:-0}"

CF="-march=native -O3"

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

# ---- resolve the base ref (fork point) if not given ----------------------
if [ -z "${BASE_REF:-}" ]; then
	for ref in origin/master master origin/main main; do
		if git rev-parse --verify -q "$ref" >/dev/null; then
			BASE_REF="$(git merge-base HEAD "$ref")"
			break
		fi
	done
fi
if [ -z "${BASE_REF:-}" ]; then
	echo "ERROR: could not auto-detect a base ref." >&2
	echo "       Set it explicitly, e.g.  BASE_REF=<commit> $0" >&2
	exit 1
fi

BASE_SHA="$(git rev-parse --short "$BASE_REF")"
BRANCH_SHA="$(git rev-parse --short "$BRANCH_REF")"

# ---- small helpers -------------------------------------------------------
# Run a build's `vips` with the dynamic loader pointed at *its own* libvips,
# so we never accidentally load a system-installed libvips (which causes
# "undefined symbol" errors from a tool/library version mismatch).
# $1 must be the path to a build's vips binary (build/tools/vips).
vrun() {
	local vips="$1"; shift
	local ld; ld="$(cd "$(dirname "$vips")/../libvips" && pwd)"
	LD_LIBRARY_PATH="$ld${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
	DYLD_LIBRARY_PATH="$ld${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}" \
		"$vips" "$@"
}

# After building, prove the tool actually loads its *own* libvips. If it
# instead resolves an older system libvips, `vips --version` fails with an
# undefined-symbol error - catch that here with a precise diagnostic.
preflight() {
	local vips="$1" label="$2" libdir
	libdir="$(cd "$(dirname "$vips")/../libvips" 2>/dev/null && pwd || true)"
	if vrun "$vips" --version >/dev/null 2>"$WORK_ROOT/last.err"; then
		return 0
	fi
	echo "" >&2
	echo "ERROR: the $label build cannot load its own libvips." >&2
	echo "--- stderr ---------------------------------------------" >&2
	cat "$WORK_ROOT/last.err" >&2
	echo "--------------------------------------------------------" >&2
	echo "  tool          : $vips" >&2
	echo "  expected lib  : $libdir" >&2
	if [ -n "$libdir" ]; then
		ls -1 "$libdir"/libvips.so* "$libdir"/libvips*.dylib 2>/dev/null |
			sed 's/^/      have: /' >&2 ||
			echo "      have: (no libvips shared object in that dir!)" >&2
	fi
	if command -v ldd >/dev/null 2>&1; then
		echo "  ldd resolves libvips to:" >&2
		LD_LIBRARY_PATH="$libdir${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
			ldd "$vips" 2>/dev/null | grep -i vips | sed 's/^/      /' >&2 ||
			echo "      (none)" >&2
	fi
	echo "  hint: if ldd points at /usr or /lib, an old system libvips is" >&2
	echo "        shadowing the build. This script forces LD_LIBRARY_PATH," >&2
	echo "        so this usually means the build's libvips.so is missing" >&2
	echo "        or in an unexpected dir - paste this output back." >&2
	exit 1
}

# run a command; on failure show the real stderr and a helpful hint, then exit.
must() {
	if ! vrun "$@" 2>"$WORK_ROOT/last.err"; then
		echo "" >&2
		echo "ERROR: command failed:" >&2
		printf '   ' >&2; printf '%q ' "$@" >&2; echo >&2
		echo "--- stderr ---------------------------------------------" >&2
		cat "$WORK_ROOT/last.err" >&2
		echo "--------------------------------------------------------" >&2
		echo "hint: if this says 'No space left on device', WORK_ROOT is" >&2
		echo "      on a small disk (e.g. a tmpfs /tmp). Re-run pointing" >&2
		echo "      it at a bigger volume, e.g.:" >&2
		echo "          WORK_ROOT=\$HOME/vips-bench $0" >&2
		echo "      or use a smaller image, e.g.  W=4000 H=4000 $0" >&2
		exit 1
	fi
}

sha() { # bit-exact hash, portable
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | awk '{print $1}'
	else
		shasum -a 256 "$1" | awk '{print $1}'
	fi
}

ncpu() {
	if command -v nproc >/dev/null 2>&1; then nproc
	elif command -v sysctl >/dev/null 2>&1; then sysctl -n hw.ncpu
	else echo "?"; fi
}

# best (min) wall-clock over $REPS runs, using the bash `time` builtin
# (portable across Linux/macOS, unlike /usr/bin/time output formats).
bench() {
	local best="" t i
	for ((i = 0; i < REPS; i++)); do
		t=$( { TIMEFORMAT=%R; time vrun "$@" >/dev/null 2>&1; } 2>&1 )
		best=$(awk -v a="$t" -v b="$best" \
			'BEGIN { if (b == "" || a + 0 < b + 0) print a; else print b }')
	done
	echo "$best"
}

speedup() { awk -v b="$1" -v k="$2" 'BEGIN { printf "%.2f", b / k }'; }

# ---- build a variant in its own worktree, run in place -------------------
# echoes the path to its `vips` binary on stdout; logs go to stderr.
build_variant() {
	local ref="$1" label="$2" sha wt
	sha="$(git rev-parse --short "$ref")"
	wt="$WORK_ROOT/$label-$sha"

	if [ "$REBUILD" = 1 ] || [ ! -x "$wt/build/tools/vips" ]; then
		echo ">>> building $label ($sha) in $wt" >&2
		rm -rf "$wt"
		git worktree prune
		git worktree add -f "$wt" "$ref" >/dev/null
		(
			cd "$wt"
			CFLAGS="$CF" CXXFLAGS="$CF" \
				meson setup build --buildtype=release >/dev/null
			ninja -C build >/dev/null
		) >&2
	else
		echo ">>> reusing existing build for $label ($sha)" >&2
	fi
	echo "$wt/build/tools/vips"
}

# ==========================================================================
echo "================================================================"
echo " libvips reducev / resize A/B benchmark"
echo "================================================================"
echo " base   : $BASE_REF  ($BASE_SHA)"
echo " branch : $BRANCH_REF  ($BRANCH_SHA)"
echo " image  : ${W}x${H}   kernel=$KERNEL   reps=$REPS (min reported)"
echo " host   : $(uname -m) $(uname -s), $(ncpu) logical CPUs"
echo " flags  : buildtype=release  CFLAGS/CXXFLAGS='$CF'"
echo "----------------------------------------------------------------"

mkdir -p "$WORK_ROOT"
echo " disk   : $(df -h "$WORK_ROOT" 2>/dev/null | awk 'NR==2 {print $4" free on "$6}')" 
echo "----------------------------------------------------------------"

BASE_VIPS="$(build_variant "$BASE_REF" base)"
BRANCH_VIPS="$(build_variant "$BRANCH_REF" branch)"

preflight "$BASE_VIPS" base
preflight "$BRANCH_VIPS" branch
echo " base   cc: $(vrun "$BASE_VIPS" --version 2>/dev/null | head -1)" >&2
echo " branch cc: $(vrun "$BRANCH_VIPS" --version 2>/dev/null | head -1)" >&2

# ---- test data (generated once, with the base build) ---------------------
DATA="$WORK_ROOT/data"
mkdir -p "$DATA"
if [ "$REBUILD" = 1 ] || [ ! -f "$DATA/complex.v" ]; then
	echo ">>> generating ${W}x${H} test images in $DATA" >&2
	must "$BASE_VIPS" gaussnoise "$DATA/base.v" "$W" "$H"
	for fmt in uchar ushort short float double; do
		must "$BASE_VIPS" cast "$DATA/base.v" "$DATA/$fmt.v" "$fmt"
	done
	# complex = two float bands joined
	must "$BASE_VIPS" complexform "$DATA/float.v" "$DATA/float.v" "$DATA/complex.v"
	echo ">>> test images ready" >&2
fi

OUT="$WORK_ROOT/out"
mkdir -p "$OUT"

# ==========================================================================
# 1. Isolated reducev - single thread isolates the inner loop
# ==========================================================================
echo
echo "== isolated 'vips reducev $VSHRINK' (VIPS_CONCURRENCY=1, min of $REPS) =="
printf "%-9s %10s %10s %9s  %s\n" format base branch speedup bit-exact
export VIPS_CONCURRENCY=1
for fmt in float complex ushort short double; do
	src="$DATA/$fmt.v"
	[ -f "$src" ] || continue
	# checked reference runs (surface real errors + produce outputs to diff)
	must "$BASE_VIPS"   reducev "$src" "$OUT/rb_$fmt.v" "$VSHRINK" --kernel "$KERNEL"
	must "$BRANCH_VIPS" reducev "$src" "$OUT/rk_$fmt.v" "$VSHRINK" --kernel "$KERNEL"
	exact=$([ "$(sha "$OUT/rb_$fmt.v")" = "$(sha "$OUT/rk_$fmt.v")" ] && echo yes || echo NO)
	b=$(bench "$BASE_VIPS"   reducev "$src" "$OUT/rb_$fmt.v" "$VSHRINK" --kernel "$KERNEL")
	k=$(bench "$BRANCH_VIPS" reducev "$src" "$OUT/rk_$fmt.v" "$VSHRINK" --kernel "$KERNEL")
	printf "%-9s %9ss %9ss %8sx  %s\n" "$fmt" "$b" "$k" "$(speedup "$b" "$k")" "$exact"
done
unset VIPS_CONCURRENCY

# ==========================================================================
# 2. End-to-end resize - all threads, real-world pipeline
# ==========================================================================
echo
echo "== end-to-end 'vips resize $SCALE' (all threads, min of $REPS) =="
printf "%-9s %10s %10s %9s  %s\n" format base branch speedup bit-exact
for fmt in uchar ushort float; do
	src="$DATA/$fmt.v"
	[ -f "$src" ] || continue
	must "$BASE_VIPS"   resize "$src" "$OUT/zb_$fmt.v" "$SCALE" --kernel "$KERNEL"
	must "$BRANCH_VIPS" resize "$src" "$OUT/zk_$fmt.v" "$SCALE" --kernel "$KERNEL"
	exact=$([ "$(sha "$OUT/zb_$fmt.v")" = "$(sha "$OUT/zk_$fmt.v")" ] && echo yes || echo NO)
	b=$(bench "$BASE_VIPS"   resize "$src" "$OUT/zb_$fmt.v" "$SCALE" --kernel "$KERNEL")
	k=$(bench "$BRANCH_VIPS" resize "$src" "$OUT/zk_$fmt.v" "$SCALE" --kernel "$KERNEL")
	printf "%-9s %9ss %9ss %8sx  %s\n" "$fmt" "$b" "$k" "$(speedup "$b" "$k")" "$exact"
done

echo
echo "----------------------------------------------------------------"
echo " done. builds cached under $WORK_ROOT (set REBUILD=1 to redo)."
echo " note: uchar routes to the Highway SIMD path in both builds, so"
echo "       it is a control and should read ~1.00x."
echo "================================================================"
