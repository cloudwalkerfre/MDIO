# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **Note**: the public repository history is a single-root snapshot — it does not
> inherit the development repository's commit history — so this file starts at
> `v0.8.5`. Earlier versions are not tracked here.

## [Unreleased]

### Added

### Changed

### Fixed

### Removed

## [0.8.5] - unreleased

First public release. **Replace the placeholder entries below before tagging.**

Platform scope: Linux x86_64 and macOS arm64. Windows is supported through WSL2,
which consumes the same Linux wheel; native Windows is not supported. Platforms
without a prebuilt wheel can build from the sdist (requires Rust, Node and
Docker).

### Added

- First public release: Markdown engine (Rust core), Python SDK / CLI, gateway,
  and Web UI (WASM).

### Changed

- Distribution name unified as `mdio-py`; the build backend switched to maturin.

### Fixed

### Removed
