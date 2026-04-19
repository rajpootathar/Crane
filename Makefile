APP_NAME  := Crane
VERSION   := $(shell awk -F'"' '/^version/ { print $$2; exit }' Cargo.toml)
BIN_NAME  := crane

ARCH      := $(shell uname -m)
TARGET_DIR := target/release
BUNDLE_DIR := $(TARGET_DIR)/bundle/osx
APP        := $(BUNDLE_DIR)/$(APP_NAME).app
DMG        := $(TARGET_DIR)/$(APP_NAME)-$(VERSION)-$(ARCH).dmg
UNIVERSAL_APP := $(TARGET_DIR)/bundle-universal/$(APP_NAME).app
UNIVERSAL_DMG := $(TARGET_DIR)/$(APP_NAME)-$(VERSION)-universal.dmg

.PHONY: help build test run release bundle dmg icns clean \
        release-universal bundle-universal dmg-universal \
        install-cargo-bundle upload

help:
	@echo "Crane — make targets"
	@echo ""
	@echo "  build              debug build"
	@echo "  test               run cargo tests"
	@echo "  run                cargo run"
	@echo ""
	@echo "  icns               regenerate icons/crane.icns from crane.png"
	@echo "  bundle             release build + .app bundle (host arch)"
	@echo "  dmg                bundle + .dmg (host arch)"
	@echo "  release            == dmg"
	@echo ""
	@echo "  bundle-universal   build arm64+x86_64, lipo into one .app"
	@echo "  dmg-universal      bundle-universal + .dmg"
	@echo "  release-universal  == dmg-universal"
	@echo ""
	@echo "  upload TAG=v0.1.0  create a GitHub release and attach the DMG"
	@echo "  clean              remove bundles and DMGs"

build:
	cargo build

test:
	cargo test --bin $(BIN_NAME)

run:
	cargo run

icns: icons/crane.icns

icons/crane.icns: crane.png scripts/make-icns.sh
	./scripts/make-icns.sh

install-cargo-bundle:
	@command -v cargo-bundle >/dev/null 2>&1 || cargo install cargo-bundle

bundle: icns install-cargo-bundle
	cargo bundle --release
	@if [ "$$(uname)" = "Darwin" ]; then \
		codesign --force --deep --sign - "$(APP)" && \
		echo "ad-hoc signed: $(APP)"; \
	fi
	@echo "bundle ready: $(APP)"

dmg: bundle
	rm -rf target/release/dmg-staging
	mkdir -p target/release/dmg-staging
	cp -R "$(APP)" target/release/dmg-staging/
	ln -s /Applications target/release/dmg-staging/Applications
	# Ad-hoc-signed builds get flagged by Gatekeeper on first launch.
	# Ship a one-click quarantine-strip helper + README alongside the
	# .app so the first-run UX is "right-click → Open" on a .command
	# file instead of five clicks through System Settings → Privacy.
	cp "scripts/dmg-assets/Fix Gatekeeper.command" target/release/dmg-staging/
	cp "scripts/dmg-assets/README - First Run.txt" target/release/dmg-staging/
	chmod +x "target/release/dmg-staging/Fix Gatekeeper.command"
	rm -f "$(DMG)"
	hdiutil create -volname "$(APP_NAME)" \
		-srcfolder target/release/dmg-staging \
		-ov -format UDZO \
		"$(DMG)"
	rm -rf target/release/dmg-staging
	@echo "dmg ready: $(DMG)"

release: dmg

bundle-universal: icns install-cargo-bundle
	rustup target add aarch64-apple-darwin >/dev/null 2>&1 || true
	rustup target add x86_64-apple-darwin >/dev/null 2>&1 || true
	cargo build --release --target aarch64-apple-darwin
	cargo build --release --target x86_64-apple-darwin
	cargo bundle --release --target aarch64-apple-darwin
	mkdir -p "$(dir $(UNIVERSAL_APP))"
	rm -rf "$(UNIVERSAL_APP)"
	cp -R "target/aarch64-apple-darwin/release/bundle/osx/$(APP_NAME).app" \
		"$(UNIVERSAL_APP)"
	lipo -create \
		"target/aarch64-apple-darwin/release/$(BIN_NAME)" \
		"target/x86_64-apple-darwin/release/$(BIN_NAME)" \
		-output "$(UNIVERSAL_APP)/Contents/MacOS/$(BIN_NAME)"
	@codesign --force --deep --sign - "$(UNIVERSAL_APP)" && \
		echo "ad-hoc signed: $(UNIVERSAL_APP)"
	@echo "universal bundle ready: $(UNIVERSAL_APP)"

dmg-universal: bundle-universal
	rm -rf target/release/dmg-staging-universal
	mkdir -p target/release/dmg-staging-universal
	cp -R "$(UNIVERSAL_APP)" target/release/dmg-staging-universal/
	ln -s /Applications target/release/dmg-staging-universal/Applications
	cp "scripts/dmg-assets/Fix Gatekeeper.command" target/release/dmg-staging-universal/
	cp "scripts/dmg-assets/README - First Run.txt" target/release/dmg-staging-universal/
	chmod +x "target/release/dmg-staging-universal/Fix Gatekeeper.command"
	rm -f "$(UNIVERSAL_DMG)"
	hdiutil create -volname "$(APP_NAME)" \
		-srcfolder target/release/dmg-staging-universal \
		-ov -format UDZO \
		"$(UNIVERSAL_DMG)"
	rm -rf target/release/dmg-staging-universal
	@echo "universal dmg ready: $(UNIVERSAL_DMG)"

release-universal: dmg-universal

upload:
ifndef TAG
	$(error TAG is required. Usage: make upload TAG=v0.1.0)
endif
	@test -f "$(DMG)" || { echo "no DMG — run 'make release' first"; exit 1; }
	gh release create "$(TAG)" "$(DMG)" \
		--title "Crane $(TAG)" \
		--notes "Crane $(VERSION) — macOS $(ARCH) preview build."

clean:
	rm -rf target/release/bundle target/*/release/bundle \
		target/release/bundle-universal \
		target/release/*.dmg
