"""Relate changed definition occurrences to the definitions and references Tiger loaded."""

from collections import defaultdict


def location(row):
    return row["from"], row["path"], row["line"], row["column"]


def file_local(change):
    return change["kind"] in ("scripted_effect", "scripted_trigger") and change["path"].startswith("events/")


def assess(change, definitions):
    """Classify a change from current resolution. Keep lost historical bindings explicit."""
    current = [row for row in definitions if row["status"] in ("effective", "contribution")]
    if change.get("file_mask"):
        return "unchanged", set()
    if file_local(change):
        # Tiger checks the global registry first. Other event files' local symbols never
        # owned references from this file, even when their spellings are identical.
        if any(row.get("visibility") != "file" for row in current):
            return "unchanged", set()
        current = [row for row in current if row["path"] == change["path"]]
    if change.get("ambiguous"):
        return "possible", {location(row) for row in current}
    sites = {(change["after_from"], change["path"], p["line"], p["column"]) for p in change["after"]}
    active = {location(row) for row in current} & sites
    if active:
        return "changed", active
    if sites:
        shadowed = {location(row) for row in definitions if row["status"] == "shadowed"}
        if not sites.issubset(shadowed):
            return "unresolved", set()
        if len(change["before"]) > len(change["after"]):
            return "possible", {location(row) for row in current}
        # A formerly winning event can lose because its body changed the override priority.
        if change["kind"] == "event" and change["before"] and (
                [p.get("priority") for p in change["before"]] !=
                [p.get("priority") for p in change["after"]]):
            return "possible", {location(row) for row in current}
        return "unchanged", set()
    if not current:
        return "removed", set()
    # The old binding is unavailable from current validation. Include surviving callers
    # for review if deleting a definition may have exposed another definition.
    return "possible", {location(row) for row in current}


def report(layer, inventory, native):
    lookups = {row["symbol"]: row for row in native["lookups"]}
    grouped = defaultdict(list)
    for change in native["diff"]["changes"]:
        grouped[change["symbol"], change["kind"]].append(change)
    symbols = []
    descriptor_changed = any(row["path"] == "descriptor.mod" for row in inventory["skipped"])
    for (symbol, kind), changes in sorted(grouped.items()):
        lookup = lookups[symbol]
        definitions = [row for row in lookup["definitions"] if row["kind"] == kind]
        decisions = set()
        targets = set()
        for change in changes:
            status, affected = assess(change, definitions)
            if descriptor_changed and status in ("unchanged", "removed"):
                # The previous descriptor may have selected a different winner.
                status = "possible"
                affected = {location(r) for r in definitions if r["status"] != "shadowed"}
            change["status"] = status
            decisions.add(status)
            targets.update(affected)
        status = next(s for s in ("changed", "possible", "unresolved", "removed", "unchanged") if s in decisions)
        callers = []
        for row in lookup["callers"]:
            if row["kind"] != kind or not targets:
                continue
            if row.get("target"):
                if location(row["target"]) not in targets:
                    continue
            elif any(d.get("visibility") == "file" for d in definitions):
                # Identically named file-local effects cannot be attributed without their target.
                continue
            callers.append(row)
        potentially_changed = [change for change in changes if change["status"] != "unchanged"]
        global_change = any(not file_local(change) for change in potentially_changed)
        local_paths = {change["path"] for change in potentially_changed if file_local(change)}
        unresolved = [row for row in lookup["unresolved_callers"]
                      if row["kind"] == kind and (global_change or row["path"] in local_paths)]
        symbols.append(dict(symbol=symbol, kind=kind, status=status, changes=changes,
                            definitions=definitions, callers=callers, unresolved_callers=unresolved))
    skipped = sorted(inventory["skipped"] + native["diff"]["skipped"], key=lambda row: row["path"])
    return dict(base=inventory["base"], layer=layer, files=[f["path"] for f in inventory["files"]],
                symbols=symbols, skipped=skipped,
                definition_coverage="Authored top-level definitions and event-file declarations in supported "
                                    "CK3 script paths. Nested or generated database keys can have other consumers.",
                caller_coverage="Direct references resolved during current validation. Removed definitions, "
                                "engine calls and unexpanded macros can have additional callers.")


def print_report(result):
    print(f'Impact against {result["base"]} in {result["layer"]}')
    for symbol in result["symbols"]:
        print(f'  {symbol["status"]} {symbol["kind"]} {symbol["symbol"]} ({len(symbol["callers"])} caller sites)')
        for change in symbol["changes"]:
            side = "after" if change["after"] else "before"
            for site in change[side]:
                print(f'    {change["status"]} {change[side + "_from"]} {change["path"]}:{site["line"]}')
            if mask := change.get("file_mask"):
                print(f'    masked by {mask["from"]} ({mask["reason"]})')
        if symbol["status"] in ("unchanged", "possible"):
            for definition in symbol["definitions"]:
                if definition["status"] in ("effective", "contribution"):
                    print(f'    {definition["status"]} {definition["from"]} '
                          f'{definition["path"]}:{definition["line"]}')
        for row in symbol["callers"]:
            print(f'    {row.get("caller", "(unknown caller)")}  {row["from"]} {row["path"]}:{row["line"]}')
        for row in symbol["unresolved_callers"]:
            print(f'    unresolved in {row.get("caller", "(unknown caller)")}  '
                  f'{row["from"]} {row["path"]}:{row["line"]}')
    if not result["symbols"]:
        print("  No changed indexed symbols")
    for skip in result["skipped"]:
        print(f'  Unindexed {skip["path"]} ({skip["reason"]})')
    print(result["caller_coverage"])
