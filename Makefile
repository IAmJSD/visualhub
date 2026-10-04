# VisualHub's build entry points. `cargo build` / `cargo run` are still
# the thing to reach for while working; this is for packaging.
#
#   make release              universal VisualHub.app, .zip and .dmg in dist/
#   make release PROFILE=dev  the same from a debug build, quicker
#   make icon                 rebuild the .icns from assets/icon.svg
#   make clean-dist           remove dist/

PROFILE ?= release
# Cargo calls the debug profile `dev` but builds it into target/debug.
ifeq ($(PROFILE),dev)
  PROFILE_DIR := debug
else
  PROFILE_DIR := $(PROFILE)
endif

.PHONY: help release icon clean-dist

help:
	@echo 'make release              universal VisualHub.app, .zip and .dmg in dist/'
	@echo 'make release PROFILE=dev  the same from a debug build'
	@echo 'make icon                 rebuild the .icns from assets/icon.svg'
	@echo 'make clean-dist           remove dist/'

release:
ifeq ($(shell uname -s),Darwin)
	@mkdir -p dist
	./packaging/macos/bundle.sh $(PROFILE_DIR)
else
	@echo 'error: packaging is macOS-only for now' >&2; exit 1
endif

icon:
	./packaging/macos/icon.sh

clean-dist:
	rm -rf dist
