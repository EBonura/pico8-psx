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
# `collection-install` target installs the collection disc here under
# COLLECTION_NAME so it shows up in PSoXide's list automatically. Override with
# `make collection-install PSOXIDE_LIB=/path`. The name has spaces; mkisopsx
# quotes the CUE FILE line.
PSOXIDE_LIB     ?= $(HOME)/Downloads/ps1 games
COLLECTION_NAME := Celeste Classic Collection

.PHONY: help submodule clean collection collection-disc collection-install collection-release celeste celeste-disc celeste2 celeste2-disc

help:
	@echo "pico8-psx targets:"
	@echo "  make collection         - build the Celeste Classic Collection launcher PSX-EXE (both games + menu)"
	@echo "  make collection-disc    - build the collection + pack dist/celeste-collection.{bin,cue}  [headline artifact]"
	@echo "  make collection-install - build the collection + install into PSoXide's library as 'Celeste Classic Collection'"
	@echo "  make celeste            - build the standalone Celeste PSX-EXE"
	@echo "  make celeste-disc       - build celeste + pack a burnable .bin/.cue into dist/"
	@echo "  make celeste2           - build the standalone Celeste 2 PSX-EXE"
	@echo "  make celeste2-disc      - build celeste2 + pack a burnable .bin/.cue into dist/"
	@echo "  make submodule          - init/update the pinned PSoXide submodule"
	@echo "  make clean              - remove build output"

submodule:
	git submodule update --init --recursive

clean:
	rm -rf $(DIST)
	cd games/celeste-collection && cargo clean
	cd games/celeste && cargo clean
	cd games/celeste2 && cargo clean

# ---- Celeste Classic Collection (headline artifact) ------------------
# One bootable image: a cover menu that launches either game. Both games
# are linked into the launcher as libraries, so this is a single boot EXE
# (mkisopsx packs exactly one).
COLLECTION_DIR := $(ROOT)/games/celeste-collection
COLLECTION_EXE := $(COLLECTION_DIR)/target/$(TARGET)/release/celeste-collection.exe

collection:
	cd $(COLLECTION_DIR) && cargo build --release
	@echo "EXE  -> $(COLLECTION_EXE)"

collection-disc: collection
	@mkdir -p $(DIST)
	cd $(MKISOPSX) && cargo run --release -- \
		--exe $(COLLECTION_EXE) \
		--out $(DIST)/celeste-collection.bin \
		--volume CELESTECOLL
	@echo "DISC -> $(DIST)/celeste-collection.cue"

# Install the collection disc into PSoXide's game library under COLLECTION_NAME,
# so it appears in the frontend's list. mkisopsx writes the .cue alongside the
# .bin with a relative FILE reference, matching the library's per-game folder
# layout. The library path may contain a space, so quote it in the recipe.
collection-install: collection
	@mkdir -p "$(PSOXIDE_LIB)/$(COLLECTION_NAME)"
	cd $(MKISOPSX) && cargo run --release -- \
		--exe $(COLLECTION_EXE) \
		--out "$(PSOXIDE_LIB)/$(COLLECTION_NAME)/$(COLLECTION_NAME).bin" \
		--volume CELESTECOLL
	@echo "INSTALLED -> $(PSOXIDE_LIB)/$(COLLECTION_NAME)/$(COLLECTION_NAME).cue"

# Stage the collection disc into release/ (a tracked dir) so committing + pushing
# triggers the itch.io upload via butler -- see .github/workflows/deploy.yml.
RELEASE := $(ROOT)/release
collection-release: collection-disc
	@mkdir -p $(RELEASE)
	cp $(DIST)/celeste-collection.bin $(DIST)/celeste-collection.cue $(RELEASE)/
	@echo "RELEASE -> $(RELEASE)/  (now: git add release && git commit && git push)"

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
