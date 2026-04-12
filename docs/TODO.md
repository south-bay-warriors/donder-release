# TODO

## Features
- Footer links support in changelog
- Add support to other git providers (GitLab, Bitbucket)
- `--bump-build` flag for build-only increments with build notes
- Rework `build_metadata` to use config-defined initial value

## Code improvements
- Support multiple footers in commit body parsing (`src/changelog.rs:107`)
- Add option to include commit body in changelog (`src/ctx.rs:91`)
- Improve version regex to handle files with multiple semver strings (`src/bump_files.rs:95`)
- Optimize pre-release tag cleanup loop for repos with many tags (`src/package.rs:439`)
