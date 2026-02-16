# Contributing to apish

Thanks for your interest in contributing to apish! 🎉

## Development Setup

### Prerequisites

- Rust 1.93.1 or later
- C compiler (gcc or clang) for building dependencies
- OpenSSL development libraries

### Ubuntu/Debian

```bash
sudo apt-get install build-essential pkg-config libssl-dev
```

### macOS

```bash
xcode-select --install
```

### Windows

Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/#build-tools-for-visual-studio-2022)

## Building

```bash
# Clone the repository
git clone https://github.com/dacort/apish.git
cd apish

# Build
cargo build

# Run
cargo run

# Build release version
cargo build --release
./target/release/apish
```

## Running Tests

```bash
cargo test
```

## Code Structure

- `src/main.rs` - Entry point and initialization
- `src/repl.rs` - Interactive shell and command processing
- `src/api.rs` - HTTP client and request handling
- `src/config.rs` - YAML configuration and history management
- `examples/` - Example API configurations

## Adding a New Feature

1. Create an issue describing the feature
2. Fork the repository
3. Create a feature branch (`git checkout -b feature/my-feature`)
4. Make your changes
5. Add tests if applicable
6. Commit your changes
7. Push to your fork
8. Open a Pull Request

## Code Style

We follow standard Rust formatting:

```bash
cargo fmt
cargo clippy
```

## Questions?

Open an issue or start a discussion!
