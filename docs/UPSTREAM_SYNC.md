# Syncing with upstream Flowsurface

Upstream: `upstream` remote, https://github.com/flowsurface-rs/flowsurface. Merge it into a
branch, never straight into `main`.

## Renamed here, keep our name when resolving conflicts

| Upstream | Here |
|---|---|
| crate `flowsurface-data` / `flowsurface_data` | `aquanter-surface-data` / `aquanter_surface_data` |
| crate `flowsurface-exchange` | `aquanter-surface-exchange` |
| crate `flowsurface-indicator-engine` | `aquanter-surface-indicator-engine` |
| binary `flowsurface-indicator` (`src/bin/flowsurface-indicator.rs`) | `aquanter-indicator` (`src/bin/aquanter-indicator.rs`) |
| env `FLOWSURFACE_DATA_PATH`, `FLOWSURFACE_INDICATORS_PATH` | `AQUANTER_SURFACE_DATA_PATH`, `AQUANTER_SURFACE_INDICATORS_PATH` |
| build env `FLOWSURFACE_GIT_SHA`, `_IS_RELEASE_TAG`, `_IS_OFFICIAL_RELEASE`, `FLOWSURFACE_OFFICIAL_RELEASE` | `AQUANTER_SURFACE_*` |
| log files `flowsurface-current.log` / `-previous.log` | `aquanter-surface-current.log` / `-previous.log` |
| `scripts/packaging/windows/flowsurface.rc` / `.manifest` | `aquanter-surface.rc` / `.manifest` |
| keychain service `flowsurface.<x>` via `keyring::Entry::new` | `data::config::keychain::entry("<x>", …)` → `aquanter-surface.<x>` |
| data dir `flowsurface/` | `aquanter-surface/` |

After a merge, this must print nothing except the intentional references listed below:

```sh
git grep -n -i flowsurface -- ':!Cargo.lock' ':!docs/UPSTREAM_SYNC.md'
```

New upstream code that calls `keyring::Entry::new("flowsurface.…")` must go through
`data::config::keychain::entry` instead, or it will miss migrated keys.

## Intentionally still `flowsurface`

- `struct Flowsurface` in `src/main.rs` (internal; renaming would conflict on every upstream merge).
- Legacy data dir migration in `data/src/lib.rs` and `LEGACY_SERVICE_PREFIX` in `data/src/config/keychain.rs`.
- Legacy theme name `"Flowsurface"` in `data/src/config/theme.rs` (maps old configs).
- `api.flowsurface.com` indicator catalog, `flowsurface-rs/*` links, licence and attribution text.
