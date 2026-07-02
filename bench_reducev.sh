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
#   W, H       8000  test image dimensions
#   VSHRINK    3.0   vertical shrink factor for the isolated reducev test
#   SCALE      0.2   scale factor for the resize test (5x downscale)
#   KERNEL     lanczos3  resample kernel
#   REPS       5     timed repetitions (best/min is reported)
#   WORK_ROOT  /tmp/vips-reducev-bench   where worktrees + builds live
#   REBUILD    0     set to 1 to rebuild even if a build already exists

set -euo pipefail

# ---- config --------------------------------------------------------------
BRANCH_REF="${BRANCH_REF:-HEAD}"
W="${W:-8000}"
H="${H:-8000}"
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
		t=$( { TIMEFORMAT=%R; time "$@" >/dev/null 2>&1; } 2>&1 )
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

BASE_VIPS="$(build_variant "$BASE_REF" base)"
BRANCH_VIPS="$(build_variant "$BRANCH_REF" branch)"

echo " base   cc: $("$BASE_VIPS" --version 2>/dev/null | head -1)" >&2

# ---- test data (generated once, with the base build) ---------------------
DATA="$WORK_ROOT/data"
mkdir -p "$DATA"
if [ "$REBUILD" = 1 ] || [ ! -f "$DATA/float.v" ]; then
	echo ">>> generating ${W}x${H} test images in $DATA" >&2
	"$BASE_VIPS" gaussnoise "$DATA/base.v" "$W" "$H" >/dev/null 2>&1
	for fmt in uchar ushort short float double; do
		"$BASE_VIPS" cast "$DATA/base.v" "$DATA/$fmt.v" "$fmt" >/dev/null 2>&1
	done
	# complex = two float bands joined
	"$BASE_VIPS" complexform "$DATA/float.v" "$DATA/float.v" \
		"$DATA/complex.v" >/dev/null 2>&1
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
	b=$(bench "$BASE_VIPS"   reducev "$src" "$OUT/rb_$fmt.v" "$VSHRINK" --kernel "$KERNEL")
	k=$(bench "$BRANCH_VIPS" reducev "$src" "$OUT/rk_$fmt.v" "$VSHRINK" --kernel "$KERNEL")
	exact=$([ "$(sha "$OUT/rb_$fmt.v")" = "$(sha "$OUT/rk_$fmt.v")" ] && echo yes || echo NO)
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
	b=$(bench "$BASE_VIPS"   resize "$src" "$OUT/zb_$fmt.v" "$SCALE" --kernel "$KERNEL")
	k=$(bench "$BRANCH_VIPS" resize "$src" "$OUT/zk_$fmt.v" "$SCALE" --kernel "$KERNEL")
	exact=$([ "$(sha "$OUT/zb_$fmt.v")" = "$(sha "$OUT/zk_$fmt.v")" ] && echo yes || echo NO)
	printf "%-9s %9ss %9ss %8sx  %s\n" "$fmt" "$b" "$k" "$(speedup "$b" "$k")" "$exact"
done

echo
echo "----------------------------------------------------------------"
echo " done. builds cached under $WORK_ROOT (set REBUILD=1 to redo)."
echo " note: uchar routes to the Highway SIMD path in both builds, so"
echo "       it is a control and should read ~1.00x."
echo "================================================================"
