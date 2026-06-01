#!/bin/bash
# test.sh — Build and run all 12 GNU Mach tests using podman.
#
# Usage:
#   ./test.sh              build image + run all 12 tests
#   ./test.sh build        only build the container image
#   ./test.sh check        only run tests (assumes image already built)
#   ./test.sh shell        drop into a shell inside the container
#   ./test.sh single NAME  run a single test (e.g. test-hello)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMAGE="gnumach-test:latest"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log_info()  { echo -e "${GREEN}[INFO]${NC}  $*"; }
log_warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
log_error() { echo -e "${RED}[ERROR]${NC} $*"; }

build_image() {
    log_info "Building podman image: ${IMAGE}"
    podman build -t "${IMAGE}" -f "${SCRIPT_DIR}/Dockerfile" "${SCRIPT_DIR}"
    log_info "Image built: ${IMAGE}"
}

run_check() {
    log_info "Running full test suite (make check)..."
    podman run --rm -v "${SCRIPT_DIR}:/src:Z" "${IMAGE}" check
    local rc=$?
    echo
    if [ $rc -eq 0 ]; then
        log_info "All 12 tests PASSED"
    else
        log_error "Test suite FAILED (exit code: $rc)"
    fi

    if [ -d "${SCRIPT_DIR}/build-docker/tests" ]; then
        echo
        log_info "Per-test summary:"
        for f in "${SCRIPT_DIR}/build-docker/tests"/test-*.trs; do
            [ -f "$f" ] || continue
            local n result
            n=$(basename "$f" .trs) || true
            result=$(grep '^:test-result:' "$f" 2>/dev/null | head -1 | sed 's/^:test-result: *//') || true

            case "${result}" in
                PASS)
                    echo -e "  ${GREEN}PASS${NC}  $n" ;;
                SKIP)
                    echo -e "  ${YELLOW}SKIP${NC}  $n" ;;
                FAIL)
                    echo -e "  ${RED}FAIL${NC}  $n" ;;
                ERROR)
                    echo -e "  ${RED}ERROR${NC} $n" ;;
                XFAIL)
                    echo -e "  ${YELLOW}XFAIL${NC} $n" ;;
                XPASS)
                    echo -e "  ${YELLOW}XPASS${NC} $n" ;;
                *)
                    # Fall back to marker inspection in .log if .trs has no result
                    local logf
                    logf="${SCRIPT_DIR}/build-docker/tests/${n}.log"
                    if [ -f "$logf" ]; then
                        if grep -q 'gnumach-test-success-and-reboot' "$logf" 2>/dev/null; then
                            echo -e "  ${GREEN}PASS${NC}  $n"
                        elif grep -q 'gnumach-test-failure' "$logf" 2>/dev/null; then
                            echo -e "  ${RED}FAIL${NC}  $n"
                        else
                            echo -e "  ${YELLOW}????${NC} $n"
                        fi
                    else
                        echo -e "  ${YELLOW}????${NC} $n"
                    fi
                    ;;
            esac
        done
    fi
    exit $rc
}

run_build() {
    log_info "Building kernel only (gnumach.o)..."
    podman run --rm -v "${SCRIPT_DIR}:/src:Z" "${IMAGE}" build
    local rc=$?
    if [ $rc -eq 0 ]; then
        log_info "Kernel build PASSED"
    else
        log_error "Kernel build FAILED (exit code: $rc)"
    fi
    exit $rc
}

run_shell() {
    log_info "Starting interactive shell..."
    podman run --rm -it -v "${SCRIPT_DIR}:/src:Z" "${IMAGE}" shell
}

run_single() {
    local name="$1"
    log_info "Running single test: ${name}"
    podman run --rm -v "${SCRIPT_DIR}:/src:Z" --entrypoint /bin/bash "${IMAGE}" -c "
        cd /src
        autoreconf -fi
        mkdir -p build-docker && cd build-docker
        ../configure --host=i686-gnu \
            CC=\"clang --target=i686-linux-gnu --gcc-toolchain=/usr -ffreestanding\" \
            LD=i686-linux-gnu-ld \
            AR=i686-linux-gnu-ar NM=i686-linux-gnu-nm \
            RANLIB=i686-linux-gnu-ranlib \
            OBJCOPY=i686-linux-gnu-objcopy \
            OBJDUMP=i686-linux-gnu-objdump \
            STRIP=i686-linux-gnu-strip
        make -j\$(nproc)
        make run-${name}
    "
    local rc=$?
    if [ $rc -eq 0 ]; then
        log_info "Test ${name} PASSED"
    else
        log_error "Test ${name} FAILED (exit code: $rc)"
    fi
    exit $rc
}

usage() {
    cat <<EOF
Usage: $0 [build|check|shell|single NAME]

Commands:
  (no arg)   Build image + run all 12 tests
  build      Only build the podman container image
  check      Run full test suite (image must exist)
  shell      Interactive shell inside container
  single N   Build and run a single test (e.g. "test-hello")

Tests: test-multiboot, test-hello, test-mach_host, test-gsync,
       test-mach_port, test-vm, test-syscalls, test-machmsg,
       test-task, test-threads, test-thread-state, test-thread-state-fp
EOF
}

if ! command -v podman >/dev/null 2>&1; then
    log_error "podman not found"
    exit 1
fi

MODE="${1:-check-build}"

case "$MODE" in
    build)          build_image ;;
    check)          run_check ;;
    check-build|"") build_image; run_check ;;
    shell)          podman image exists "${IMAGE}" 2>/dev/null || build_image; run_shell ;;
    single)         [ $# -ge 2 ] || { log_error "usage: $0 single <test-name>"; exit 1; }
                    podman image exists "${IMAGE}" 2>/dev/null || build_image
                    run_single "$2" ;;
    -h|--help|help) usage; exit 0 ;;
    *)              log_error "Unknown: $MODE"; usage; exit 1 ;;
esac
