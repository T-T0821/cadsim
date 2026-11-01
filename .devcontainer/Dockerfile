# plantwright の開発コンテナ。VS Code の Dev Containers と、ホストからの `make docker-<目標>` の両方で使う。
# Rust のツールチェーン、Bevy が Linux で要るライブラリ、`cargo ui-check` 用の仮想画面 (Xvfb) と
# ソフトウェア描画 (Mesa の lavapipe)、Windows 向けのクロスビルド (MinGW) を入れる。
# 社内プロキシの証明書を入れた像などに差し替えるときは --build-arg BASE_IMAGE=... を渡す
ARG BASE_IMAGE=ubuntu:24.04
FROM ${BASE_IMAGE}

ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        curl ca-certificates \
        # ビルド (openh264 は C++、lzma-sys は C をソースからビルドする)
        build-essential pkg-config cmake nasm clang \
        # Bevy: 音 (ALSA)、ゲームパッド (udev)、窓 (X11 / Wayland)
        libasound2-dev libudev-dev \
        libx11-dev libxcursor-dev libxi-dev libxrandr-dev libxkbcommon-dev libxkbcommon-x11-0 \
        libwayland-dev \
        # 描画: Vulkan のローダーと Mesa (GPU がなくても lavapipe で描ける)
        libvulkan1 mesa-vulkan-drivers vulkan-tools libgl1 libegl1 \
        # cargo ui-check: 画面がなくても仮想画面で動かす
        xvfb xauth \
        # Windows 向けのクロスビルドと配布用の zip
        mingw-w64 zip \
        # 開発に使う道具
        git make sudo less \
    && rm -rf /var/lib/apt/lists/*

# Rust。版を固定して、誰の環境でも同じコンパイラでビルドする
ARG RUST_VERSION=1.97.1
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
# Apple Silicon の Mac などでも動くよう、CPU (x86_64 / aarch64) に合った rustup を取ってくる
RUN set -eu; \
    arch="$(uname -m)"; \
    curl --proto '=https' --tlsv1.2 -sSfL -o /tmp/rustup-init \
        "https://static.rust-lang.org/rustup/dist/${arch}-unknown-linux-gnu/rustup-init"; \
    chmod +x /tmp/rustup-init; \
    /tmp/rustup-init -y --no-modify-path --profile minimal --default-toolchain "$RUST_VERSION" \
        --component rustfmt,clippy --target x86_64-pc-windows-gnu; \
    rm /tmp/rustup-init; \
    rustc --version

# Windows 向けの exe が libstdc++-6.dll などを探さずに 1 つで動くよう、MinGW の C++ 標準ライブラリの
# DLL 用の取り込みライブラリを消して、静的ライブラリ (.a) がリンクされるようにする
RUN find /usr/lib/gcc/x86_64-w64-mingw32 -name 'libstdc++.dll.a' -delete

# ホストからの `make docker-<目標>` は、ホストの利用者の uid で動かす。
# どの uid でも依存クレートを取ってこられるように、cargo のホームを誰でも書けるようにしておく。
RUN mkdir -p "$CARGO_HOME/registry" "$CARGO_HOME/git" \
    && chmod -R a+rwX "$CARGO_HOME" "$RUSTUP_HOME"

# Dev Containers で使う一般ユーザー (uid はホストに合わせて自動で付け替えられる)
ARG USERNAME=dev
ARG USER_UID=1000
ARG USER_GID=$USER_UID
# Ubuntu の像に最初からいる ubuntu (uid 1000) は消して、同じ uid を dev に使う
RUN userdel -r ubuntu 2>/dev/null || true \
    && groupadd --gid "$USER_GID" "$USERNAME" \
    && useradd --uid "$USER_UID" --gid "$USER_GID" -m -s /bin/bash "$USERNAME" \
    && echo "$USERNAME ALL=(root) NOPASSWD:ALL" > /etc/sudoers.d/"$USERNAME" \
    && chmod 0440 /etc/sudoers.d/"$USERNAME"

# ホストの target/ (Windows / Mac でのビルド結果) とぶつからないよう、コンテナでのビルド結果は分けて置く
ENV CARGO_TARGET_DIR=/workspace/target/container

WORKDIR /workspace
