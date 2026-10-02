# Hermetic environment for LEVEL 1 + LEVEL 2 tests (no printer, no host packages).
#   docker build -t canon-print-test -f docker/Dockerfile .
#   scripts/test-docker.sh
FROM rust:1-bookworm

RUN apt-get update \
 && apt-get install -y --no-install-recommends libusb-1.0-0-dev pkg-config usbutils \
 && rm -rf /var/lib/apt/lists/* \
 && rustup component add rustfmt clippy

# keep the container's build output separate from the host's target/
ENV CARGO_TARGET_DIR=/tmp/target
WORKDIR /src
