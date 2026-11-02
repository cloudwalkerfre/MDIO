# Third-party notices

MDIO is distributed under the MIT license (see `LICENSE`). This project also
contains or links third-party components that keep their own licenses. This file
lists them so the notices travel with the source and binary distributions.

## Bundled stylesheets and fonts

| Component | Location | License | Copyright |
|---|---|---|---|
| Mdmdt theme | `mdio_ui/lib/mods/ui_std/src/themes/vendor/mdmdt-*.css` | Apache-2.0 | see `vendor/Mdmdt-LICENSE.txt` |
| Bloom theme | `mdio_ui/lib/mods/ui_std/src/themes/vendor/bloom-mist*.css` | MIT | Copyright (c) 2025 webkubor |
| github-markdown theme | `mdio_ui/lib/mods/ui_std/src/themes/vendor/github-markdown.css` | MIT | Copyright (c) Sindre Sorhus <sindresorhus@gmail.com> |
| macos27 theme | `mdio_ui/lib/mods/ui_std/src/themes/vendor/macos27.css` | MIT | Copyright (c) 2026 christoarmani |
| JetBrains Mono | `mdio_ui/lib/mods/ui_std/src/themes/fonts/jetbrains-mono-*.woff2` | SIL OFL 1.1 | Copyright 2020 The JetBrains Mono Project Authors |

The full license texts for the four themes are kept next to the files in
`mdio_ui/lib/mods/ui_std/src/themes/vendor/`. The OFL text for JetBrains Mono is
in `mdio_ui/lib/mods/ui_std/src/themes/fonts/OFL.txt`.

## Bundled documents

| Component | Location | License | Copyright |
|---|---|---|---|
| CommonMark specification 0.31.2 | `commonmark.md` | **CC-BY-SA 4.0** | John MacFarlane |

`commonmark.md` is the CommonMark specification used as a test fixture. **It is
not covered by the project's MIT license**; it stays under CC-BY-SA 4.0, and any
redistribution must keep that license and attribution. The attribution and
license notice for it is in `commonmark-LICENSE.txt`.

## Bundled demo assets

| Component | Location | License | Copyright | License file |
|---|---|---|---|---|
| Monaco Editor (ESM build + CSS) | `mdio_ui/demo/vendor/monaco.api.*` | MIT | Copyright (c) Microsoft Corporation | `mdio_ui/demo/vendor/LICENSE-monaco.txt` |
| Babel standalone | `mdio_ui/demo/vendor/babel-standalone.mjs` | MIT | Copyright (c) 2014-present Sebastian McKenzie and other contributors | `mdio_ui/demo/vendor/LICENSE-babel.txt` |
| Courier Prime (font) | `mdio_ui/demo/ui/CourierPrime-*.ttf` | SIL OFL 1.1 | Copyright (c) 2013, Quote-Unquote Apps, with Reserved Font Name "Courier Prime" | `mdio_ui/demo/ui/OFL.txt` |

## Linked at build time

| Component | How it is used | License |
|---|---|---|
| DuckDB | `mdio_frame.duckdb_extension` is built from `mdio_core/ext/arrow` against the `duckdb-neo` and `duckdb-loadable-macros` crates and links DuckDB | MIT |
| crop | Text rope, linked into `mdio_core`: the document body is held as a `crop::Rope` | MIT |
| cola | Text CRDT, linked into `mdio_core`: collaborative editing sessions are `cola::Replica`s | MIT |

DuckDB is loaded as a separate extension at runtime; the DuckDB binary itself is
not bundled by this project.

## License file locations

Standalone license texts ship next to the files they cover:

- `mdio_ui/demo/ui/OFL.txt` — Courier Prime (SIL OFL 1.1)
- `mdio_ui/demo/vendor/LICENSE-monaco.txt` — Monaco Editor (MIT)
- `mdio_ui/demo/vendor/LICENSE-babel.txt` — Babel standalone (MIT)
- `commonmark-LICENSE.txt` — CommonMark specification (CC BY-SA 4.0)
- `mdio_ui/lib/mods/ui_std/src/themes/fonts/OFL.txt` — JetBrains Mono (SIL OFL 1.1)
- `mdio_ui/lib/mods/ui_std/src/themes/vendor/*-LICENSE.txt` — the four themes
  (Mdmdt, bloom, github-markdown, macos27)

## Dependencies installed from package registries

Python packages (PyPI), Rust crates (crates.io) and npm packages are **not**
listed here: each distribution carries its own license metadata, and package
managers surface it (`uv.lock`, `Cargo.lock`, `pnpm-lock.yaml`). This file covers
only content vendored into the repository or linked into shipped binaries.
