# Pull local, gitignored overrides (DEVELOPER_ID, NOTARY_PROFILE, etc).
# Safe when absent — `-include` never errors on a missing file.
-include .env.local

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
        install-cargo-bundle upload \
        sign sign-universal notarize notarize-universal \
        staple staple-universal signed-dmg signed-dmg-universal \
        setup-notary \
        bump-patch bump-minor tag ship ship-universal

# Developer ID signing + notarization.
#
#   DEVELOPER_ID  — exact keychain identity name, e.g.
#                   "Developer ID Application: Your Name (TEAMID)"
#   NOTARY_PROFILE — keychain-profile name used by `xcrun notarytool`.
#                    Set up once via: make setup-notary
#   ENTITLEMENTS  — plist path (defaults to scripts/entitlements.plist)
DEVELOPER_ID   ?=
NOTARY_PROFILE ?= crane-notary
ENTITLEMENTS   ?= scripts/entitlements.plist

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
	@echo ""
	@echo "  bump-patch         bump Cargo.toml patch (0.1.72 → 0.1.73) and commit"
	@echo "  bump-minor         bump Cargo.toml minor (0.1.72 → 0.2.0)  and commit"
	@echo "  tag                create & push the git tag vX.Y.Z for the current Cargo.toml"
	@echo "  ship               bump-patch + release + tag + upload  (one-shot release)"
	@echo "  ship-universal     bump-patch + release-universal + tag + upload (universal)"
	@echo ""
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
	@if [ "$$(uname)" = "Darwin" ] && [ -z "$(DEVELOPER_ID)" ]; then \
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
	@if [ -z "$(DEVELOPER_ID)" ]; then \
		codesign --force --deep --sign - "$(UNIVERSAL_APP)" && \
		echo "ad-hoc signed: $(UNIVERSAL_APP)"; \
	fi
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

# ---------------------------------------------------------------------------
# Signing / notarization
# ---------------------------------------------------------------------------

# One-time: store an app-specific password in keychain under $(NOTARY_PROFILE).
# You'll be prompted for Apple ID, Team ID, and the app-specific password
# generated at appleid.apple.com → Sign-In and Security → App-Specific Passwords.
setup-notary:
	@command -v xcrun >/dev/null 2>&1 || { echo "xcrun not found — install Xcode CLT"; exit 1; }
	xcrun notarytool store-credentials "$(NOTARY_PROFILE)" \
		--apple-id "$${APPLE_ID:-$$(read -p 'Apple ID: ' v; echo $$v)}" \
		--team-id  "$${TEAM_ID:-$$(read -p  'Team ID:  ' v; echo $$v)}"

# Deep-sign a bundle with the Developer ID cert + hardened runtime +
# secure timestamp. Required for notarization.
define _sign_bundle
	@test -n "$(DEVELOPER_ID)" || { echo "DEVELOPER_ID is not set — see 'make help'"; exit 1; }
	@test -f "$(ENTITLEMENTS)" || { echo "entitlements plist missing: $(ENTITLEMENTS)"; exit 1; }
	@# Sign nested binaries/frameworks first, outward to the app.
	find "$(1)/Contents" -type f \( -name "*.dylib" -o -name "*.so" -o -perm +111 \) \
		! -path "$(1)/Contents/MacOS/*" \
		-exec codesign --force --timestamp --options runtime --sign "$(DEVELOPER_ID)" {} \; || true
	codesign --force --timestamp --options runtime \
		--entitlements "$(ENTITLEMENTS)" \
		--sign "$(DEVELOPER_ID)" "$(1)/Contents/MacOS"/* || true
	codesign --force --timestamp --options runtime \
		--entitlements "$(ENTITLEMENTS)" \
		--sign "$(DEVELOPER_ID)" "$(1)"
	codesign --verify --deep --strict --verbose=2 "$(1)"
	@echo "signed: $(1)"
endef

sign: bundle
	$(call _sign_bundle,$(APP))

sign-universal: bundle-universal
	$(call _sign_bundle,$(UNIVERSAL_APP))

# Submit a DMG for notarization and wait for the ticket. The DMG itself
# must be built from an already-signed .app. On success, stapler-staple
# so the ticket travels with the DMG (offline install works).
define _notarize_dmg
	@test -f "$(1)" || { echo "missing DMG: $(1) — run the matching signed-dmg target first"; exit 1; }
	xcrun notarytool submit "$(1)" \
		--keychain-profile "$(NOTARY_PROFILE)" \
		--wait
	xcrun stapler staple "$(1)"
	xcrun stapler validate "$(1)"
	spctl --assess --type open --context context:primary-signature -v "$(1)" || true
	@echo "notarized + stapled: $(1)"
endef

notarize:
	$(call _notarize_dmg,$(DMG))

notarize-universal:
	$(call _notarize_dmg,$(UNIVERSAL_DMG))

staple:
	xcrun stapler staple "$(DMG)"

staple-universal:
	xcrun stapler staple "$(UNIVERSAL_DMG)"

# End-to-end: build → sign app → build DMG from signed app → notarize → staple.
signed-dmg: sign dmg notarize

signed-dmg-universal: sign-universal dmg-universal notarize-universal

# ---------------------------------------------------------------------------
# Release workflow
# ---------------------------------------------------------------------------
# `make ship` is the one-button release: bump patch, build DMG, tag, push,
# attach DMG to a GitHub release. Use this instead of hand-rolling
# `sed s/version/ + cargo build + git push + gh release create`.

# Refuse to release on a dirty tree — uncommitted work would ship as
# part of the tag but never land on main.
_check_clean:
	@git diff-index --quiet HEAD -- || \
		{ echo "working tree is dirty; commit or stash before releasing"; exit 1; }

_bump_%:
	@cur=$$(awk -F'"' '/^version/ { print $$2; exit }' Cargo.toml) ; \
	case "$*" in \
	  patch) next=$$(awk -F. '{ printf "%d.%d.%d", $$1,$$2,$$3+1 }' <<<"$$cur");; \
	  minor) next=$$(awk -F. '{ printf "%d.%d.0",  $$1,$$2+1     }' <<<"$$cur");; \
	  *)     echo "unknown bump kind: $*"; exit 1;; \
	esac ; \
	echo "bump: $$cur → $$next" ; \
	sed -i '' -E "s/^version = \"$$cur\"/version = \"$$next\"/" Cargo.toml ; \
	cargo build --quiet ; \
	git add Cargo.toml Cargo.lock 2>/dev/null || git add Cargo.toml ; \
	git commit -m "chore(crane): v$$next"

bump-patch: _check_clean _bump_patch
bump-minor: _check_clean _bump_minor

# Tag the current Cargo.toml version and push it. Idempotent-ish —
# errors cleanly if the tag already exists locally.
tag:
	@v=$$(awk -F'"' '/^version/ { print $$2; exit }' Cargo.toml) ; \
	t="v$$v" ; \
	if git rev-parse -q --verify "refs/tags/$$t" >/dev/null ; then \
		echo "tag $$t already exists locally"; exit 1 ; \
	fi ; \
	git tag -a "$$t" -m "Crane $$t" && \
	git push origin main "$$t" && \
	echo "pushed tag $$t"

# One-shot release. Bumps patch → builds DMG → tags → pushes tag →
# uploads DMG as a GitHub release. Abort anywhere and fix, then re-run.
ship: bump-patch release tag
	@v=$$(awk -F'"' '/^version/ { print $$2; exit }' Cargo.toml) ; \
	$(MAKE) upload TAG="v$$v"

ship-universal: bump-patch release-universal tag
	@v=$$(awk -F'"' '/^version/ { print $$2; exit }' Cargo.toml) ; \
	test -f "$(UNIVERSAL_DMG)" || { echo "universal DMG missing"; exit 1; } ; \
	gh release create "v$$v" "$(UNIVERSAL_DMG)" \
		--title "Crane v$$v" \
		--notes "Crane $$v — universal (arm64 + x86_64) build."
