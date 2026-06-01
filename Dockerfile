# Dockerfile for building and testing GNU Mach (i686 target)
#
# Usage:
#   docker build -t gnumach-build .
#
#   docker run --rm -v $(pwd):/src gnumach-build build
#       Compile the kernel (gnumach.o), surfacing all compile errors.
#
#   docker run --rm -v $(pwd):/src gnumach-build check
#       Full kernel build + run the QEMU test suite.
#
#   docker run --rm -it -v $(pwd):/src gnumach-build shell
#       Interactive shell for manual builds and debugging.
#
# MIG (Mach Interface Generator) is installed from Debian (mig-i686-gnu:i386),
# eliminating the need for a multi-stage bootstrap.

FROM docker.io/library/debian:trixie-slim

LABEL maintainer="GNU Mach Developers"
LABEL description="Build and test environment for GNU Mach kernel (i686 target)"

ENV DEBIAN_FRONTEND=noninteractive

# Enable the i386 architecture so we can install mig-i686-gnu:i386
RUN dpkg --add-architecture i386

# Install all build and test dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    # Autotools build system
    autoconf \
    automake \
    make \
    patch \
    gzip \
    file \
    # i686 cross-compiler toolchain (binutils + libc headers; clang is the C
    # compiler, but we still need binutils for ld/ar/etc. and the
    # libc6-dev-i386-cross package for the multilib headers).  Trixie's
    # clang (>= 19) supports `-std=c23` natively.
    clang \
    clang-format \
    lld \
    llvm \
    gcc-i686-linux-gnu \
    binutils-i686-linux-gnu \
    libc6-dev-i386-cross \
    # Mach Interface Generator for i686 (from Debian)
    mig-i686-gnu:i386 \
    # MIG wrapper script needs perl for path resolution
    perl \
    # Test suite: QEMU + GRUB ISO creation
    qemu-system-x86 \
    grub-common \
    grub-pc-bin \
    xorriso \
    # GNU AWK (required by gensym.awk for kernel symbol generation)
    gawk \
    # Texinfo: regenerate doc/mach.info during full `make` builds.
    texinfo \
    # Utilities
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Install Rust (stable) and the i586 cross-compilation target
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --default-toolchain stable --no-modify-path
ENV PATH="/root/.cargo/bin:${PATH}"
RUN rustup target add i586-unknown-linux-gnu

# Sanity-check that key tools are available and functional
# (i686-gnu-mig doesn't support --version, so we test existence + basic invocation)
RUN command -v i686-gnu-mig && i686-gnu-mig < /dev/null \
    && clang --version \
    && i686-linux-gnu-ld --version \
    && make --version \
    && autoconf --version \
    && qemu-system-i386 --version \
    && grub-mkrescue --help > /dev/null

# Source code is mounted at runtime
WORKDIR /src

# Entrypoint dispatches to build.sh based on user command
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["shell"]
