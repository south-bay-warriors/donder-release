<p align="center">
  <img src="https://user-images.githubusercontent.com/12548890/204842249-c59bf410-2ab2-41c7-8f1a-bbb7aa1f5275.png" style="border-radius: 8px;">
  <h1 align="center">donder-release</h1>
  <h3 align="center">Quickly create releases on Github from the command line or CI using conventional commits.</h3>
</p>

<br />

## Installation

#### With Cargo

```bash
cargo install donder-release
```

#### With Npm

```bash
npm install -g donder-release-cli
```

## Quick start

Initialize a configuration file in your project:

```bash
donder-release --init
```

This creates a `donder-release.yaml` file with commented examples for all options.

Preview a release without publishing:

```bash
donder-release --dry-run
```

Publish a release:

```bash
donder-release
```

## CLI options

```
-i, --init           Initialize configuration file
-c, --config <FILE>  Configuration file path [default: donder-release.yaml]
-p, --packages       Comma-separated list of monorepo packages to release
    --pre-id <ID>    Pre-release identifier (e.g. alpha, beta, rc)
    --dry-run        Preview a pending release without publishing
-v, --version        Output CLI version
```

## Configuration

All configuration is done in `donder-release.yaml`.

### release_message

The commit message for the release commit. Use `%s` as a placeholder for the version.

```yaml
release_message: "chore(release): %s"
```

### tag_prefix

Prefix for release tags.

```yaml
tag_prefix: v # creates tags like v1.0.0
```

### types

Commit types that trigger a release. `feat`, `fix` and `revert` are reserved types with fixed bump rules (`feat` = minor, `fix`/`revert` = patch). You can override their section names and add custom types with `minor` or `patch` bumps.

```yaml
types:
  - { commit_type: feat, section: Features }
  - { commit_type: fix, section: Bug Fixes }
  - { commit_type: perf, bump: patch, section: Performance Improvements }
```

### bump_files

Files to update with the new version. At least one must be defined. Supported targets: `cargo`, `npm`, `pub`, `android`, `ios`.

```yaml
bump_files:
  - { target: cargo, path: <root> }
  - { target: npm, path: <root> }
  - { target: pub, path: <root> }
  - { target: android, path: android }
  - { target: ios, path: ios/MyApp }
```

Use `<root>` to target the directory where donder-release is executed.

#### iOS and App Store Connect

App Store Connect only allows version numbers as 3 period-separated integers, no pre-release or build metadata strings. To work around this, donder-release translates pre-release identifiers into numeric values for `CURRENT_PROJECT_VERSION`:

| Pre-release             | CURRENT_PROJECT_VERSION |
| ----------------------- | ----------------------- |
| `alpha.N`               | `1.N`                   |
| `beta.N`                | `2.N`                   |
| `rc.N`                  | `3.N`                   |
| Other                   | `4.N`                   |
| Stable (no pre-release) | `5.0`                   |

`MARKETING_VERSION` is always set to the semver version without pre-release info (e.g. `1.2.0`).

For example, a release of `1.2.0-beta.3` sets:

- `MARKETING_VERSION = 1.2.0`
- `CURRENT_PROJECT_VERSION = 2.3`

#### Build metadata

Append an auto-incrementing build number to the version:

```yaml
bump_files:
  - { target: npm, path: <root>, build_metadata: true } # e.g. 1.0.0+1, 1.0.0+2
```

Build metadata must be numeric. For `android` and `ios` targets, the build number is always incremented automatically.

### changelog_file

Write release notes to a changelog file:

```yaml
changelog_file: CHANGELOG.md
```

### clean_pre_releases

Delete pre-release tags and GitHub releases when a stable release is published:

```yaml
clean_pre_releases: true
```

### Monorepo support

Mark bump files as individual packages to release them independently. Each package gets its own tags, changelog, and version based on commits under its directory.

```yaml
bump_files:
  - { target: npm, path: packages/api, package: true }
  - { target: npm, path: packages/web, package: true }
```

Release specific packages:

```bash
donder-release --packages api,web
```

## Pre-releases

Create pre-release versions with the `--pre-id` flag:

```bash
donder-release --pre-id alpha  # 1.0.1-alpha.0
donder-release --pre-id beta   # 1.0.1-beta.0
donder-release --pre-id rc     # 1.0.1-rc.0
```

Subsequent pre-releases auto-increment: `1.0.1-alpha.0` -> `1.0.1-alpha.1`.

## Environment variables

| Variable              | Description                                            | Default                             |
| --------------------- | ------------------------------------------------------ | ----------------------------------- |
| `GH_TOKEN`            | GitHub personal access token (required for publishing) |                                     |
| `GIT_AUTHOR_NAME`     | Git author name                                        | `sbayw-bot`                         |
| `GIT_AUTHOR_EMAIL`    | Git author email                                       | `<sbayw-bot current primary email>` |
| `GIT_COMMITTER_NAME`  | Git committer name                                     | Falls back to `GIT_AUTHOR_NAME`     |
| `GIT_COMMITTER_EMAIL` | Git committer email                                    | Falls back to `GIT_AUTHOR_EMAIL`    |

Environment variables can be set in a `donder-release.env` file in your project root.

#### GH_TOKEN permissions

The `GH_TOKEN` must be a personal access token with **Contents: Read and write** permission on the target repository. This allows donder-release to push commits, tags, and create GitHub releases.

If the current branch is protected, the user that created the PAT must be added to the branch protection bypass list (**Settings > Rules > Rulesets** or **Settings > Branches > Allow specified actors to bypass required pull requests**).

## CI / GitHub Actions

Example workflow for manual releases:

```yaml
name: Release

on:
  workflow_dispatch:
    inputs:
      type:
        description: Release type
        required: true
        type: choice
        options:
          - release
          - beta

jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
        with:
          fetch-depth: 0
          persist-credentials: false

      - uses: dtolnay/rust-toolchain@stable

      - run: cargo install donder-release

      - name: Run donder-release
        run: |
          if [ "${{ inputs.type }}" = "beta" ]; then
            donder-release --pre-id beta
          else
            donder-release
          fi
        env:
          GH_TOKEN: ${{ secrets.GH_TOKEN }}
```

> **Note:** Use `persist-credentials: false` on checkout so donder-release authenticates with your `GH_TOKEN` instead of the default `GITHUB_TOKEN`.

## Conventional Commits

donder-release follows the [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/) specification:

```
<type>(<optional scope>): <description>

[optional body]

[optional footer(s)]
```

Version bumps are determined by commit type:

| Commit                    | Version bump           |
| ------------------------- | ---------------------- |
| `fix: ...`                | Patch (1.0.0 -> 1.0.1) |
| `feat: ...`               | Minor (1.0.0 -> 1.1.0) |
| `BREAKING CHANGE:` footer | Major (1.0.0 -> 2.0.0) |

#### TODO

- Footer links support
- Add support to other git providers(?)
