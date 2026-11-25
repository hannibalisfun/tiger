# CK3 1.21 first pass

This branch starts from pharaox's `adapt/ck3-1.20.1.1` at `9f61d918`. Its first pass covers the 1.21 province-to-district links, district back-links, the regular and owned trade-post district iterators, and the receiver and argument types for `add_building`. It also reports a CK3 `trigger_else_if` chain that ends or is interrupted before a sibling `trigger_else`. The inherited faith and rite loaders support PoD's hunter faiths. The district and building validators also cover holding definitions, building needs and market modifiers, district assets and construction callbacks, and dated province history. They resolve item references across the game and ordered parents.

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
py -3 -B workspace/tests/port_passes.py --pass districts --workspace .. --game $game --tiger target/release/ck3-tiger.exe
py -3 -B workspace/tests/test_runner.py
cargo test --locked -p tiger-lib
```

The live acceptance command checks the exact five fixture diagnostics and their source lines, seven optional Silver Pack references with Camp Pressure and PoD alone, and zero warnings or errors with all three siblings. The fixture also exercises the valid district and trigger forms. The runner tests use a temporary game fixture to exercise baseline reuse, invalidation, and failure handling. These checks require no changes to the installed game, the reference snapshots, or the mods under test.

The schema fixtures use the public runner and PoD as a parent. Each expected error has a source marker, diagnostic key and message check. Any additional warning or error fails the pass.
