FROM rust:1.92-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends \
    xvfb xclip sway wl-clipboard dbus-x11 python3-pil meson libwayland-dev wayland-protocols \
    && rm -rf /var/lib/apt/lists/*
RUN git clone --depth 1 --branch v2.3.0 https://github.com/bugaevc/wl-clipboard /tmp/wl-clipboard \
    && meson setup /tmp/wl-clipboard/build /tmp/wl-clipboard \
    && ninja -C /tmp/wl-clipboard/build install \
    && rm -rf /tmp/wl-clipboard
RUN useradd -m -u 1000 tester
RUN rustup component add clippy
WORKDIR /work
COPY . .
RUN cargo test --locked --no-run && cargo build --locked \
    && chown -R tester:tester /work
USER tester
ENV HOME=/home/tester
CMD ["bash", "tests/linux/run.sh"]
