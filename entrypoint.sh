#!/bin/bash
# Entrypoint for the GNU Mach build container.
#
# Dispatches the user command to build.sh or drops into a shell.
#
# Usage (via docker run):
#   docker run --rm -v $(pwd):/src gnumach-build build
#   docker run --rm -v $(pwd):/src gnumach-build check
#   docker run --rm -it -v $(pwd):/src gnumach-build shell

set -e

MODE="${1:-shell}"

case "$MODE" in
    build|check)
        exec /src/build.sh "$MODE"
        ;;
    shell|bash|sh)
        [ $# -gt 0 ] && shift
        exec /bin/bash "$@"
        ;;
    *)
        echo "Usage: docker run ... gnumach-build [build|check|shell]" >&2
        echo "  build  - compile the kernel (gnumach.o), surfacing all compile errors" >&2
        echo "  check  - full kernel build + run the QEMU test suite" >&2
        echo "  shell  - interactive shell (default)" >&2
        exit 2
        ;;
esac
