# FamilyCom — LAN Messenger
#
# Build and install targets for the FamilyCom daemon and TUI client.
#
# Usage:
#   make              # Build debug binaries
#   make release      # Build optimized release binaries
#   make install      # Build release + install to ~/.local/bin/ + set up autostart
#   make uninstall    # Remove installed binaries and autostart config
#   make test         # Run all tests
#   make clippy       # Run clippy lints
#   make clean        # Remove build artifacts

# Where to install binaries. ~/.local/bin/ is standard for user-local binaries
# and is usually in $PATH on modern Linux distributions.
PREFIX ?= $(HOME)/.local
BINDIR := $(PREFIX)/bin

# Cargo commands
CARGO := cargo
CARGO_BUILD := $(CARGO) build --workspace
CARGO_TEST := $(CARGO) test --workspace
CARGO_CLIPPY := $(CARGO) clippy --workspace -- -D warnings

.PHONY: all build release test clippy clean install uninstall help

## Default target: build debug binaries
all: build

## Build the workspace in debug mode (fast compilation, unoptimized)
build:
	$(CARGO_BUILD)

## Build the workspace in release mode (slow compilation, optimized)
release:
	$(CARGO_BUILD) --release

## Run all tests across the workspace
test:
	$(CARGO_TEST)

## Run clippy linter (strict: warnings are errors)
clippy:
	$(CARGO_CLIPPY)

## Remove build artifacts (target/ directory)
clean:
	$(CARGO) clean

## Build release binaries, install to ~/.local/bin/, and set up autostart
install: release
	@echo "Installing FamilyCom binaries to $(BINDIR)/"
	@mkdir -p $(BINDIR)
	cp target/release/familycomd $(BINDIR)/familycomd
	cp target/release/familycom $(BINDIR)/familycom
	@echo ""
	@echo "Setting up autostart..."
	$(BINDIR)/familycomd install
	@echo ""
	@echo "Installation complete!"
	@echo "  Daemon:  $(BINDIR)/familycomd"
	@echo "  TUI:     $(BINDIR)/familycom"
	@echo ""
	@echo "Make sure $(BINDIR) is in your PATH."

## Remove installed binaries and autostart configuration
uninstall:
	@echo "Removing autostart configuration..."
	-$(BINDIR)/familycomd uninstall 2>/dev/null || true
	@echo "Removing binaries..."
	rm -f $(BINDIR)/familycomd
	rm -f $(BINDIR)/familycom
	@echo "Uninstallation complete."

## Show available targets
help:
	@echo "FamilyCom Makefile targets:"
	@echo ""
	@echo "  make            Build debug binaries"
	@echo "  make release    Build optimized release binaries"
	@echo "  make test       Run all tests"
	@echo "  make clippy     Run clippy lints"
	@echo "  make clean      Remove build artifacts"
	@echo "  make install    Build release + install to $(BINDIR)/ + autostart"
	@echo "  make uninstall  Remove binaries and autostart config"
	@echo "  make help       Show this help"
