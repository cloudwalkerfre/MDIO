## What and why

<!-- One paragraph. Link the issue this comes from: "Closes #123". -->

## How it was tested

<!-- The commands you ran and the platform, e.g.
     MDIO_HOME="$(mktemp -d)" just test-py   (macOS arm64)
     just build-ui && just verify            (Ubuntu 24.04) -->

## Checklist

- [ ] One concern per pull request (no unrelated refactors or reformatting)
- [ ] `just test` passes locally
- [ ] A line was added under `## [Unreleased]` in `CHANGELOG.md`
- [ ] Commits are signed off (`git commit -s`)
- [ ] UI change: `just build-ui` ran and a screenshot is attached
- [ ] No maintainer-owned files were touched (versions, `scripts/public_release/`, `.github/workflows/`, `mdio_core/ext/arrow/`)

## Human note

<!-- Write this yourself, in your own words: what you changed and why.
     An empty note, or one that reads like generated text, may be closed without review. -->
