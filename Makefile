# VisualHub's build entry points. `cargo build` / `cargo run` are still
# the thing to reach for while working; this is for packaging.
#
#   make release              package for this machine into dist/:
#                               macOS    universal VisualHub.app, .zip and .dmg
#                               Linux    AppImage, .deb, .rpm and .pkg.tar.zst
#                               Windows  VisualHub-VERSION-setup.exe (needs NSIS)
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
	@echo 'make release              package for this machine into dist/'
	@echo 'make release PROFILE=dev  the same from a debug build'
	@echo 'make icon                 rebuild the .icns from assets/icon.svg'
	@echo 'make clean-dist           remove dist/'

VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
UNAME := $(shell uname -s)

release:
	@mkdir -p dist
ifeq ($(UNAME),Darwin)
	./packaging/macos/bundle.sh $(PROFILE_DIR)
else ifeq ($(UNAME),Linux)
	./packaging/linux/appimage.sh
	./packaging/linux/packages.sh
else ifneq (,$(findstring MINGW,$(UNAME))$(findstring MSYS,$(UNAME)))
	cargo build --release
	makensis -DVERSION=$(VERSION) packaging/windows/installer.nsi
else
	@echo 'error: nothing to package on $(UNAME)' >&2; exit 1
endif

icon:
	./packaging/macos/icon.sh

clean-dist:
	rm -rf dist
