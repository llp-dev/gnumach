#!/bin/bash
# Build and test GNU Mach for i686 inside the Docker container.
#
# MIG is provided by the Debian mig-i686-gnu:i386 package, so we can
# skip the multi-stage bootstrap and go straight to configure + build.
#
# Usage: build.sh [build|check]
#   build (default) — `make -k gnumach.o` so every compile error surfaces
#   check           — full kernel build + `make check` (QEMU test suite)
#
# Build dir: /src/build-docker. Logs in build-docker/build.log (and check.log).

set -e

MODE="${1:-build}"
case "$MODE" in
    build|check) ;;
    *) echo "build.sh: unknown mode '$MODE' (use build or check)" >&2; exit 2 ;;
esac

BUILDDIR=/src/build-docker

# Fail fast if MIG is not available (should be installed in the Docker image)
if ! command -v i686-gnu-mig >/dev/null 2>&1; then
    echo "build.sh: i686-gnu-mig not found in PATH" >&2
    echo "build.sh: install mig-i686-gnu:i386 or set MIG= to the correct path" >&2
    exit 1
fi

cd /src

# Bootstrap autotools (fast no-op if files are already up to date)
autoreconf -fi

# Configure for i686 — single stage: MIG is already installed from apt
rm -rf "$BUILDDIR"
mkdir -p "$BUILDDIR"
cd "$BUILDDIR"

# MIG is auto-detected by configure (AC_CHECK_PROG for i686-gnu-mig)
# Compiler: the i686-linux-gnu GCC cross-compiler.  GCC is the upstream-
# supported toolchain for the GNU Mach C sources, which use GNU C extensions
# (nested functions, inline-asm lvalue casts, __asm, …) that clang rejects.
# Using GCC lets the C kernel build unmodified; the Rust pmap is added on top.
../configure --host=i686-gnu \
    CC="i686-linux-gnu-gcc -ffreestanding" \
    LD=i686-linux-gnu-ld \
    AR=i686-linux-gnu-ar \
    NM=i686-linux-gnu-nm \
    RANLIB=i686-linux-gnu-ranlib \
    OBJCOPY=i686-linux-gnu-objcopy \
    OBJDUMP=i686-linux-gnu-objdump \
    STRIP=i686-linux-gnu-strip

if [ "$MODE" = build ]; then
    # -k keeps going past compile failures so we see every error, not just
    # the first one. Tee to build.log for inspection on the host.
    set +e
    make -k -j"$(nproc)" gnumach.o 2>&1 | tee build.log
    RC=${PIPESTATUS[0]}
    set -e

    echo
    echo "=== build exit code: $RC ==="
    echo "=== full log: $BUILDDIR/build.log ==="
    exit "$RC"
fi

# MODE=check: full build, then run the QEMU test suite.
set +e
make -k -j"$(nproc)" 2>&1 | tee build.log
BUILD_RC=${PIPESTATUS[0]}
set -e

if [ "$BUILD_RC" -ne 0 ]; then
    echo
    echo "=== build FAILED (exit code: $BUILD_RC) ==="
    echo "=== full log: $BUILDDIR/build.log ==="
    exit "$BUILD_RC"
fi

echo
echo "=== running make check ==="
set +e
make check 2>&1 | tee check.log
RC=${PIPESTATUS[0]}
set -e

echo
echo "=== check exit code: $RC ==="
echo "=== logs: $BUILDDIR/build.log, $BUILDDIR/check.log ==="
echo "=== per-test logs: $BUILDDIR/tests/test-*.log ==="
exit "$RC"
