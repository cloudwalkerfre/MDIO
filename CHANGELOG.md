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

## [0.8.5] - 2026-11-03

First public release.

Platform scope: Windows is supported through WSL2, which consumes the same
Linux wheel as native Linux; native Windows is not supported. The 0.8.5 PyPI
release ships the Linux x86_64 wheel (`manylinux_2_34`); the macOS arm64 wheel
follows within this version.

### Added

- First public release: Markdown engine (Rust core), Python SDK / CLI, gateway,
  and Web UI (WASM).

### Changed

- Distribution name unified as `mdio-py`; the build backend switched to maturin.

### Fixed

- The published wheel uses a single `manylinux_2_34` platform tag; PyPI rejects
  `linux_x86_64`, and a compressed `manylinux_2_34.manylinux_2_39` tag is not
  installable by pip.
- The release smoke test boots the gateway with `mdio serve`; `mdio start` no
  longer exists.
- The macOS test job runs the Python suite as several short-lived processes: a
  single pytest process exhausts the hosted runner's thread limit around 450
  tests and later setups fail to spawn threads.
- Chunk collection only accepts real test ids, so a warning line containing
  `::` can no longer turn into a bogus test target.

### Removed
