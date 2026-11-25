# CK3 1.21 port and workspace runner

This branch starts from pharaox's `adapt/ck3-1.20.1.1` at `9f61d918`. The workspace uses the CK3 1.21.0 schemas and engine exports and runs the acceptance tests against 1.21.0.3. The port is partial.

| Area | Checks |
| --- | --- |
| Districts and buildings | District and holding definitions, building needs and modifiers, scoped construction, assets, and province history across dates and load layers. |
| Trade and roads | Goods, categories, county potential goods, toll levels, trader tasks, goods graphics, roads, and road modifiers. Script checks cover market and route operations, monopolies, typed iterator filters, and scope transitions. |
| Variable maps | Scoped, global, and local storage, key and value lookups, iterators, duration, and quoted macro arguments. Local maps track writes and clears in the call context. Persistent maps combine known write types and leave dynamic writes unconstrained. |
| GUI data types | The five 1.21 engine exports supply types, functions, promotes and argument counts. Literal database getters resolve through the load order. Known province and character arguments on building and district methods are type-checked. |
| Earlier cases | District construction scope and complete `trigger_else_if` chains. The inherited faith and rite loaders resolve PoD's hunter faiths. |

## Build and run

Run commands from the Tiger repository root. The runner requires Python 3.11 or newer and accepts a CK3 install root containing `game`, `clausewitz`, `jomini`, and the launcher version file. The local Windows build helper uses Cargo on PATH or the portable compiler and locked dependency cache prepared under the workspace's `tmp/Tiger-CK3-build-tools`.

```powershell
.\workspace\build.ps1
$game = 'H:/SteamLibrary/steamapps/common/Crusader Kings III'
py -3 workspace/run.py --game $game --mod '../Camp-Pressure-PoD' --parent '../POD Reference'
```

On other machines with Rust installed, build with `cargo build --locked --release -p ck3-tiger --bin ck3-tiger`. The runner finds that release binary by default. Use `--tiger PATH` to select another build of this port. The `build-info` command prints the embedded revision and target rules. Runner JSON also records the binary SHA-256, which distinguishes locally modified builds made at the same commit.

For **base PoD**, pass the working PoD tree as `--mod`. For a **submod or drop-in**, pass its directory as `--mod` and specify each `--parent` in ascending load order. Use the actual sibling combination you intend to support. The runner loads each tree's `descriptor.mod` with its replacement paths. For a bare drop-in directory it creates a temporary descriptor. English localization and Tiger's PoD checks run in each configuration.

```powershell
py -3 workspace/run.py --game $game --mod '../Camp-Pressure-PoD' --parent '../POD Reference' --parent '../Splat-Contracts-PoD' --parent '../Patron-Offices-PoD' --parent '../PoD-Silver-Pack-Landless' --json
```

Exit **0** means the result has no findings at the selected severity, **1** means findings reached the threshold, and **2** means validation could not complete. The default threshold is `warning`. Use `--json` for Tiger's diagnostic keys, source locations, caller chains, and run metadata. The regular single-mod run uses parallel validation. `--deterministic` selects fixed hash seeds and a single validation thread.

## Find a definition and its callers

Pass an identifier to the same load order used for validation. Names match exactly, with CK3's numeric event IDs treating `tiger.1` and `tiger.0001` as the same event. This example finds the combat effect that selects the next actor in the working combat integration.

```powershell
py -3 workspace/run.py --game $game --mod '../POD-Combat-Integration/devpod' --lookup pod_combat_select_next_actor_effect
```

Add `--json` for `lookup.definitions` and `lookup.callers`. Definitions include the source layer, relative path, line, kind, status and macro parameters. Tiger marks the active definition as `effective`, additive fragments such as on-actions as `contribution`, and displaced definitions as `shadowed`. The reason distinguishes an overwritten key, a file with the same path in a higher layer, and a descriptor's `replace_path`.

Tiger retains shadow history for scripted effects, scripted triggers, script values, events, on-actions, traits, and the common item database. Other specialized registries can provide their retained keys. Caller rows identify references that Tiger resolved during validation, including macro invocation locations when expanded. Dynamically assembled identifiers that Tiger cannot resolve and engine-only calls can have no caller row. An empty caller list alone cannot establish that a symbol is unused.

Each run includes `metadata.input_revisions` in load order, with the input path, containing Git repository, full HEAD revision and dirty state. For a nested mod such as `devpod`, dirty state covers tracked changes and non-ignored untracked files under that mod directory. A non-Git game install has a launcher version and null Git fields. A Git-ignored input may have an enclosing revision and unknown dirty state. In lookup mode exit **0** means Tiger indexed an active definition or contribution, **1** means the identifier is absent or fully shadowed, and **2** indicates a runner or Tiger failure.

## Compare revisions

Pass an unmodified checkout or overlay of the primary mod as `--baseline`. Put the changed version in `--mod`. Both runs use the same game, ordered parents, and deterministic validation. The comparison includes loaded parent reports so a changed definition can surface an error in an unchanged caller. Use `--show-vanilla` when vanilla callers also need checking.

```powershell
py -3 workspace/run.py --game $game --parent '../POD Reference' --baseline 'C:/work/original-overlay' --mod 'C:/work/edited-overlay' --cache-dir target/workspace-baselines --json
```

The result contains added diagnostics and a count of removed ones. Matching uses severity, confidence, key, message and detail, source layer, relative path, source line text, and caller chain, preserving the number of occurrences. A simple line-number shift matches the original finding. All scope diagnostic categories participate. The deterministic mode builds on pharaox's experiments with validation order. [Upstream reproducibility issue](https://github.com/amtep/tiger/issues/325)

Caching is opt-in. The baseline key includes the binary and rules, game version, load order, input paths, repository revisions when available, and a file inventory of the game, shared engine data, parents and baseline. The default inventory compares names, sizes and timestamps. For archives or tools that preserve metadata while rewriting bytes, use `--cache-check content` to hash file contents as well, or `--refresh-baseline` after the refresh. Content mode reads the entire input set. Place the cache outside the mod and game trees.

## Verify this port

```powershell
py -3 -B workspace/tests/first_pass.py --workspace .. --game $game --tiger target/release/ck3-tiger.exe
py -3 -B workspace/tests/port_passes.py --workspace .. --game $game --tiger target/release/ck3-tiger.exe
py -3 -B workspace/tests/test_runner.py
py -3 -B workspace/tests/test_lookup.py
cargo test --locked -p tiger-lib
```

The live acceptance command checks the exact five fixture diagnostics and their source lines, seven optional Silver Pack references with Camp Pressure and PoD alone, and zero warnings or errors with all three siblings. The fixture also exercises the valid district and trigger forms. The runner tests use a temporary game fixture to exercise baseline reuse, invalidation, and failure handling. These checks require no changes to the installed game, the reference snapshots, or the mods under test.

To rerun one area, add `--pass districts`, `--pass trade`, `--pass maps`, or `--pass gui` to `port_passes.py`. The schema fixtures use the public runner and PoD as a parent. Each expected error has a source marker, diagnostic key and message check. Any additional warning or error fails the pass.

Road definitions are checked against province IDs. Binary spline geometry requires a separate check. For `has_trade_good_type` the export declares a trade-good receiver and describes a trade route. Neither local source tree contains a resolving call, so Tiger uses the declared receiver. GUI arguments that the engine exports only as `Arg0`, `Arg1`, and so on retain unknown types unless a verified signature is available.

### Reference-file checks on 1.21.0.3

These runs copied the named reference files into a temporary primary mod at their original relative paths. Vanilla probes used the installed game alone. PoD probes also loaded PoD as the parent.

| Primary files | Shared runner result |
| --- | --- |
| The ten shipped definitions under `common/trade`, `common/trade_tasks`, `common/trade_goods_graphics` and `common/roads` | Zero warnings or errors. |
| PoD `POD_grand_city_effects.txt` | 21 warnings or errors before the map and prosperity changes, zero after. |
| Vanilla `gui/window_trade_good.gui` | 28 data-function warnings or errors before the GUI refresh, zero after. |
| PoD `00_PoD_regular_districts.txt` and the Grand City primary-building and holding-district files | Two missing references to `district_civic`, both in the regular districts file. |
| All 85 vanilla building files, 11 district files and one holdings file | Zero unknown fields or tokens. The 604 remaining warnings and errors cover modifier use areas, tooltip complexity, asset or format references, scopes, and duplicate fields. |
