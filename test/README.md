# libvips test suite

This is in two parts: a few simple bash scripts in this directory are run on
"meson test", and a fancier Python test suite that's run by GitHub actions on
each commit.

`abi/check-abi.sh` checks that a change keeps libvips binary compatible: it
builds a base git ref and the working tree, then compares exported symbols
and public types with abidiff, compares the operations, arguments and enums
bindings see at runtime, and runs the base Python test suite on the new
library. GitHub actions runs it on each commit, see `.github/workflows/abi.yml`.
Run it locally on Linux with eg. `test/abi/check-abi.sh all origin/master`.
