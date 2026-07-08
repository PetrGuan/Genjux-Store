# `genjux.yaml` curator overlay schema

`genjux.yaml` is a manual override file for a single repo, used when the
automatic classification pipeline (extension mapping, keyword/arch fallback,
content sniffing — see `.copilot-workflow/PLAN.md` section 3) can't
correctly classify a release asset, or when a project needs custom
install metadata (minimum OS version, silent-install flags).

This is tier 4 of the classification pipeline: it only runs *after* tiers
1-3, and only overrides the fields it explicitly sets.

## Schema

```yaml
assets:
  "<exact release asset filename>":
    platform: macos | windows | linux | android
    arch: x86_64 | arm64
    min_os_version: "<free-form string, e.g. \"12.0\">"
    silent_install_args: "<free-form string, passed to the platform adapter>"
```

- Keys under `assets` are matched by **exact filename**, not a glob or
  regex. This is the simplest option that covers Phase 0's needs; if a
  curated project's asset names change every release (e.g. embed the
  version number) such that exact-match becomes impractical, pattern-based
  matching can be layered on top of this schema later without breaking
  existing `genjux.yaml` files (all fields would remain valid, only the key
  matching semantics would need to expand).
- All fields are optional. Only the fields present in an entry are applied;
  anything else about the package (including what the automatic pipeline
  already determined) is left untouched.
- Unrecognized `platform`/`arch` values are ignored rather than treated as
  a parse error, so a typo in one entry doesn't break loading the rest of
  the file.

## Example

```yaml
assets:
  "myapp-latest.zip":
    platform: macos
    arch: arm64
    min_os_version: "12.0"
    silent_install_args: "--silent --no-prompt"
```

## Where these files will live

This issue only implements the schema and loader (`load_overlay` /
`apply_overlay` in `core/src/curator.rs`, parsing a `genjux.yaml` string).
*Where* the curated `genjux.yaml` files for real projects are stored and
fetched from (e.g. a directory in this repo, a separate curated-metadata
repo, or fetched per-repo from the target project itself) is a decision
for whichever later issue wires this into the full resolve pipeline — not
decided here.
