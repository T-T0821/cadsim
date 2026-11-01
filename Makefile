# cadsim の開発用 Makefile
#
#   make build            開発ビルド             make test     テスト
#   make run ARGS="..."   起動 (引数はアプリへ)  make check    PR を出す前の確認 (fmt・clippy・test)
#   make help             目標の一覧
#
# どの目標も頭に docker- を付けると、開発コンテナ (.devcontainer/Dockerfile) の中で動かす。
# Rust や Linux のライブラリをホストに入れなくても、Docker だけあればビルドできる。
#   make docker-build     make docker-test     make docker-windows     make docker-shell

CARGO      ?= cargo
# ビルド結果の置き場所。開発コンテナの中では target/container (ホストのビルド結果と混ぜない)
TARGET_DIR := $(or $(CARGO_TARGET_DIR),target)
APP        := cad_proto
# 配布物の名前に使う版。既定は Cargo.toml の version (例: make package-windows VERSION=0.1.5)
VERSION    ?= $(shell sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n1)
DIST       := dist
WIN_TARGET := x86_64-pc-windows-gnu
# make run / make ui-check でアプリに渡す引数
ARGS       ?=

.DEFAULT_GOAL := help

.PHONY: help build release run test fmt fmt-check clippy check ui-check windows \
        package package-linux package-windows clean

help: ## この一覧を出す
	@grep -hE '^[a-zA-Z0-9_%-]+:.*## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*## "}; {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'

build: ## 開発ビルド
	$(CARGO) build --locked

release: ## リリースビルド (最適化あり)
	$(CARGO) build --release --locked

run: ## 開発ビルドで起動 (例: make run ARGS="samples/demo_line.cadsim")
	$(CARGO) run --locked -- $(ARGS)

test: ## テストを流す
	$(CARGO) test --locked

fmt: ## 書式を整える
	$(CARGO) fmt --all

fmt-check: ## 書式が整っているかを確かめる
	$(CARGO) fmt --all --check

clippy: ## clippy (警告もエラー扱い)
	$(CARGO) clippy --all-targets --locked -- -D warnings

check: fmt-check clippy test ## PR を出す前の確認 (fmt-check・clippy・test)

ui-check: ## 画面での動作確認 (画面がなければ仮想画面で。例: make ui-check ARGS="--only chart")
	tools/ui_check.sh $(ARGS)

windows: ## Windows 向けの exe をクロスビルドする (Linux で。MinGW が要る)
	$(CARGO) build --release --locked --target $(WIN_TARGET)

package: package-linux package-windows ## Linux と Windows の配布物を dist/ に作る

package-linux: release ## Linux 向けの配布物 (dist/cadsim-v<版>-linux-<CPU>.tar.gz)
	@set -e; \
	name=cadsim-v$(VERSION)-linux-$$(uname -m); \
	rm -rf "$(DIST)/$$name"; mkdir -p "$(DIST)/$$name"; \
	cp "$(TARGET_DIR)/release/$(APP)" README.md "$(DIST)/$$name/"; \
	tar -C $(DIST) -czf "$(DIST)/$$name.tar.gz" "$$name"; \
	rm -rf "$(DIST)/$$name"; \
	echo "$(DIST)/$$name.tar.gz"

package-windows: windows ## Windows 向けの配布物 (Release と同じ形の .exe と .zip)
	@set -e; \
	name=cadsim-v$(VERSION)-windows-x86_64; \
	exe="$(TARGET_DIR)/$(WIN_TARGET)/release/$(APP).exe"; \
	rm -rf "$(DIST)/$$name" "$(DIST)/$$name.zip"; mkdir -p "$(DIST)/$$name"; \
	cp "$$exe" "$(DIST)/$$name.exe"; \
	cp "$$exe" README.md "$(DIST)/$$name/"; \
	(cd $(DIST) && zip -qr "$$name.zip" "$$name"); \
	rm -rf "$(DIST)/$$name"; \
	echo "$(DIST)/$$name.exe"; echo "$(DIST)/$$name.zip"

clean: ## ビルド結果と dist/ を消す
	$(CARGO) clean
	rm -rf $(DIST)

# ---- 開発コンテナ経由 ----------------------------------------------------------------

DOCKER            ?= docker
IMAGE             ?= cadsim-dev
# docker build / docker run に足す引数 (例: DOCKER_BUILD_ARGS="--build-arg BASE_IMAGE=...")
DOCKER_BUILD_ARGS ?=
DOCKER_RUN_ARGS   ?=
# 端末から叩いたときだけ -it を付ける (CI などでは付けない)
DOCKER_TTY        := $(shell [ -t 0 ] && echo -it)
DOCKER_RUN = $(DOCKER) run --rm $(DOCKER_TTY) \
	-v "$(CURDIR)":/workspace -w /workspace \
	-v cadsim-cargo-registry:/usr/local/cargo/registry \
	-v cadsim-cargo-git:/usr/local/cargo/git \
	--user "$$(id -u):$$(id -g)" -e HOME=/tmp \
	$(DOCKER_RUN_ARGS) $(IMAGE)

.PHONY: docker-image docker-shell

docker-image: ## 開発コンテナの像を作る (2回目からはキャッシュで一瞬)
	$(DOCKER) build $(DOCKER_BUILD_ARGS) -t $(IMAGE) .devcontainer

docker-shell: docker-image ## 開発コンテナの中でシェルを開く
	$(DOCKER_RUN) bash

docker-%: docker-image ## 目標を開発コンテナの中で動かす (例: make docker-build, make docker-package-windows)
	$(DOCKER_RUN) make $* $(if $(ARGS),ARGS='$(ARGS)') $(if $(filter command line,$(origin VERSION)),VERSION=$(VERSION))
