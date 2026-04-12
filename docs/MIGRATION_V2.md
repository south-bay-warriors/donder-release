# Migrating from v1 to v2

## Breaking changes

### `--init` replaced by `--setup` and `--skip-interactive`

v1:
```bash
donder-release --init
```

v2:
```bash
# Interactive wizard
donder-release --setup

# Write default config without prompts (same as v1 --init)
donder-release --skip-interactive
```

### New configuration fields

v2 adds optional fields to `donder-release.yaml`. Existing v1 configs work without changes since all new fields have defaults.

```yaml
# New in v2 (all optional)
versioning: semver              # "semver" (default) or "calver"
calver_format: YYYY.MM.MICRO   # Required when versioning is "calver"
```

### Per-package versioning in monorepos

v2 supports per-package versioning overrides. Packages without overrides inherit the global setting.

```yaml
versioning: semver
bump_files:
  - { target: npm, path: packages/api, package: true }
  - { target: npm, path: packages/app, package: true, versioning: calver, calver_format: YYYY.MM.MICRO }
```

## New features

### Calendar versioning (CalVer)

Date-based versioning for projects with time-driven releases:

```yaml
versioning: calver
calver_format: YYYY.MM.MICRO
```

See the [README](../README.md) for supported formats and details.

### Interactive setup wizard

`--setup` walks through configuration interactively:
- Versioning scheme (semver/calver)
- Tag prefix
- Bump file targets
- Monorepo packages with per-package versioning
- Custom commit types
- Changelog and pre-release settings
