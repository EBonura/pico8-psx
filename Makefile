# pico8-psx -- PICO-8 demakes for the PlayStation 1, built on the
# PSoXide Rust SDK (pinned as a git submodule under third_party/).
#
# Each game is its own standalone Cargo workspace under games/<name>.
# A plain `cargo build --release` inside a game dir already produces a
# PSX-EXE (see that game's .cargo/config.toml + build.rs); these
# targets add disc packing via the submodule's mkisopsx tool.

ROOT     := $(CURDIR)
PSOXIDE  := $(ROOT)/third_party/PSoXide
MKISOPSX := $(PSOXIDE)/tools/mkisopsx
TARGET   := mipsel-sony-psx
DIST     := $(ROOT)/dist

# Where PSoXide's frontend scans for games (the `game_library` setting). The
# `collection` target installs the demo disc here under COLLECTION_NAME so it
# shows up in PSoXide's list automatically. Override with `make collection
# PSOXIDE_LIB=/path`. The name has spaces/parens; mkisopsx quotes the CUE FILE line.
PSOXIDE_LIB     ?= $(HOME)/Downloads/ps1 games
COLLECTION_NAME := Celeste Classic Collection PSX (Homebrew)

.PHONY: help submodule clean demo demo-disc collection celeste celeste-disc celeste2 celeste2-disc

help:
	@echo "pico8-psx targets:"
	@echo "  make demo          - build the demo-disc launcher PSX-EXE (both games + menu)"
	@echo "  make demo-disc     - build the demo + pack dist/demo.{bin,cue}  [headline artifact]"
	@echo "  make collection    - build the demo + install into PSoXide's library as 'Celeste Classic Collection PSX (Homebrew)'"
	@echo "  make celeste       - build the standalone Celeste PSX-EXE"
	@echo "  make celeste-disc  - build celeste + pack a burnable .bin/.cue into dist/"
	@echo "  make celeste2      - build the standalone Celeste 2 PSX-EXE"
	@echo "  make celeste2-disc - build celeste2 + pack a burnable .bin/.cue into dist/"
	@echo "  make submodule     - init/update the pinned PSoXide submodule"
	@echo "  make clean         - remove build output"

submodule:
	git submodule update --init --recursive

clean:
	rm -rf $(DIST)
	cd games/demo && cargo clean
	cd games/celeste && cargo clean
	cd games/celeste2 && cargo clean

# ---- Demo disc (headline artifact) -----------------------------------
# One bootable image: a cover menu that launches either game. Both games
# are linked into the launcher as libraries, so this is a single boot EXE
# (mkisopsx packs exactly one).
DEMO_DIR := $(ROOT)/games/demo
DEMO_EXE := $(DEMO_DIR)/target/$(TARGET)/release/demo.exe

demo:
	cd $(DEMO_DIR) && cargo build --release
	@echo "EXE  -> $(DEMO_EXE)"

demo-disc: demo
	@mkdir -p $(DIST)
	cd $(MKISOPSX) && cargo run --release -- \
		--exe $(DEMO_EXE) \
		--out $(DIST)/demo.bin \
		--volume PICO8PSX
	@echo "DISC -> $(DIST)/demo.cue"

# Install the demo disc into PSoXide's game library under COLLECTION_NAME,
# so it appears in the frontend's list. mkisopsx writes the .cue alongside the
# .bin with a relative FILE reference, matching the library's per-game folder
# layout. The library path may contain a space, so quote it in the recipe.
collection: demo
	@mkdir -p "$(PSOXIDE_LIB)/$(COLLECTION_NAME)"
	cd $(MKISOPSX) && cargo run --release -- \
		--exe $(DEMO_EXE) \
		--out "$(PSOXIDE_LIB)/$(COLLECTION_NAME)/$(COLLECTION_NAME).bin" \
		--volume CELESTECOLL
	@echo "INSTALLED -> $(PSOXIDE_LIB)/$(COLLECTION_NAME)/$(COLLECTION_NAME).cue"

# ---- Celeste ---------------------------------------------------------
CELESTE_DIR := $(ROOT)/games/celeste
CELESTE_EXE := $(CELESTE_DIR)/target/$(TARGET)/release/celeste.exe

celeste:
	cd $(CELESTE_DIR) && cargo build --release
	@echo "EXE  -> $(CELESTE_EXE)"

celeste-disc: celeste
	@mkdir -p $(DIST)
	cd $(MKISOPSX) && cargo run --release -- \
		--exe $(CELESTE_EXE) \
		--out $(DIST)/celeste.bin \
		--volume PICO8PSX
	@echo "DISC -> $(DIST)/celeste.cue"

# ---- Celeste 2 -------------------------------------------------------
CELESTE2_DIR := $(ROOT)/games/celeste2
CELESTE2_EXE := $(CELESTE2_DIR)/target/$(TARGET)/release/celeste2.exe

celeste2:
	cd $(CELESTE2_DIR) && cargo build --release
	@echo "EXE  -> $(CELESTE2_EXE)"

celeste2-disc: celeste2
	@mkdir -p $(DIST)
	cd $(MKISOPSX) && cargo run --release -- \
		--exe $(CELESTE2_EXE) \
		--out $(DIST)/celeste2.bin \
		--volume PICO8PSX
	@echo "DISC -> $(DIST)/celeste2.cue"
