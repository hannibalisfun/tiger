//! References resolved by the CK3 validators during an exact symbol lookup.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::everything::Everything;
use crate::item::Item;
use crate::lookup::{LookupQueries, location};
use crate::macros::MACRO_MAP;
use crate::on_action_reachability::FiringSites;
use crate::pathtable::PathTableIndex;
use crate::token::{Loc, Token};

#[derive(Debug)]
pub(crate) struct CallerLookup {
    pub(crate) queries: Arc<LookupQueries>,
    pub(crate) include_unresolved: bool,
    pub(crate) firings: Option<FiringSites>,
    hits: Mutex<BTreeMap<(usize, Resolution, Item, Loc), BTreeSet<Loc>>>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Resolution {
    Resolved,
    Unresolved,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CallerResults {
    pub(crate) resolved: Vec<Value>,
    pub(crate) unresolved: Vec<Value>,
}

impl CallerLookup {
    pub(crate) fn new(queries: Arc<LookupQueries>) -> Self {
        Self { queries, include_unresolved: false, firings: None, hits: Mutex::default() }
    }

    fn record(
        &self,
        indices: &[usize],
        resolution: Resolution,
        kind: Item,
        loc: Loc,
        target: Option<Loc>,
    ) {
        let mut hits = self.hits.lock().unwrap();
        for &index in indices {
            hits.entry((index, resolution, kind, loc)).or_default().extend(target);
        }
    }

    pub(crate) fn results(&self, data: &Everything) -> Vec<CallerResults> {
        let hits = self.hits.lock().unwrap();
        let owners = Owners::for_sites(data, hits.keys().map(|(_, _, _, loc)| *loc));

        // Path-table indices depend on file registration order. Sort by public locations
        // (including the invocation chain) for identical results across worker counts.
        let mut rows = vec![BTreeMap::new(); self.queries.len()];
        for (&(index, resolution, kind, loc), targets) in hits.iter() {
            let mut sort_key = Vec::new();
            let mut sites = expansion_chain(loc).map(|site| {
                sort_key.push((site.pathname(), site.kind, site.line, site.column));
                let mut value = location(data, site);
                if let Some((kind, name)) = owners.at(site) {
                    value["caller"] = name.into();
                    value["caller_kind"] = <&'static str>::from(kind).into();
                }
                value
            });
            let mut row = sites.next().unwrap();
            row["kind"] = <&'static str>::from(kind).into();
            row["expansion"] = sites.collect::<Vec<_>>().into();
            if targets.is_empty() {
                rows[index].insert((resolution, sort_key, kind, None), row);
            } else {
                for target in targets {
                    let mut resolved = row.clone();
                    resolved["target"] = location(data, *target);
                    let target_key = (target.pathname(), target.kind, target.line, target.column);
                    rows[index]
                        .insert((resolution, sort_key.clone(), kind, Some(target_key)), resolved);
                }
            }
        }
        rows.into_iter()
            .map(|rows| {
                let mut sites = CallerResults::default();
                for ((resolution, _, _, _), row) in rows {
                    match resolution {
                        Resolution::Resolved => sites.resolved.push(row),
                        Resolution::Unresolved => sites.unresolved.push(row),
                    }
                }
                sites
            })
            .collect()
    }
}

#[derive(Default)]
pub(crate) struct Owners(BTreeMap<PathTableIndex, Vec<(Loc, Item, &'static str)>>);

impl Owners {
    pub(crate) fn for_sites(data: &Everything, sites: impl Iterator<Item = Loc>) -> Self {
        let mut owners = Self::default();
        let mut paths = BTreeSet::new();
        for site in sites.flat_map(expansion_chain) {
            owners.0.entry(site.idx).or_default();
            paths.insert(site.pathname());
        }
        if paths.is_empty() {
            return owners;
        }
        let mut add = |kind, key: &Token| {
            if let Some(keys) = owners.0.get_mut(&key.loc.idx) {
                keys.push((key.loc, kind, key.as_str()));
            }
        };
        // Only top-level declarations can own a call site.
        for kind in [
            Item::ScriptedEffect,
            Item::ScriptedTrigger,
            Item::ScriptValue,
            Item::Event,
            Item::CharacterInteraction,
            Item::Decision,
            Item::Building,
            Item::DistrictType,
            Item::Story,
            Item::Scheme,
            Item::ScriptedGui,
            Item::ActivityType,
            Item::Trait,
        ] {
            if paths.iter().any(|path| path.starts_with(Path::new(kind.path()))) {
                for key in data.iter_keys(kind) {
                    add(kind, key);
                }
            }
        }
        for (kind, key) in data.events.local_script_keys() {
            add(kind, key);
        }
        for key in data.on_actions.fragment_keys() {
            add(Item::OnAction, key);
        }
        for keys in owners.0.values_mut() {
            keys.sort_unstable_by_key(|(loc, _, _)| (loc.line, loc.column));
        }
        owners
    }

    pub(crate) fn at(&self, site: Loc) -> Option<(Item, &'static str)> {
        let keys = self.0.get(&site.idx)?;
        let index = keys.partition_point(|(start, _, _)| {
            (start.line, start.column) <= (site.line, site.column)
        });
        let (_, kind, name) = *keys.get(index.checked_sub(1)?)?;
        Some((kind, name))
    }
}

pub(crate) fn expansion_chain(loc: Loc) -> impl Iterator<Item = Loc> {
    std::iter::successors(Some(loc), |site| site.link_idx.and_then(|idx| MACRO_MAP.get_loc(idx)))
}

impl Everything {
    /// Keep rejected, exact references separately for a changed-symbol impact report.
    /// Enable this after selecting the queries and before validation.
    pub fn enable_unresolved_callers(&mut self) {
        if let Some(lookup) = self.lookup_callers.as_mut() {
            lookup.include_unresolved = true;
        }
    }

    pub(crate) fn record_unresolved_caller(&self, kind: Item, token: &Token) {
        if let Some(lookup) = &self.lookup_callers
            && lookup.include_unresolved
        {
            let indices = lookup.queries.indices(kind, token.as_str());
            if !indices.is_empty() {
                lookup.record(indices, Resolution::Unresolved, kind, token.loc, None);
            }
        }
    }

    pub(crate) fn record_caller(&self, kind: Item, token: &Token, target: Option<&Token>) {
        if let Some(lookup) = &self.lookup_callers {
            let indices = lookup.queries.indices(kind, token.as_str());
            if !indices.is_empty() {
                lookup.record(
                    indices,
                    Resolution::Resolved,
                    kind,
                    token.loc,
                    target.map(|key| key.loc),
                );
            }
        }
    }

    pub(crate) fn record_typed_caller(&self, kind: Item, key: &str, token: &Token) {
        if let Some(lookup) = &self.lookup_callers {
            let indices = lookup.queries.indices(kind, key);
            if !indices.is_empty()
                && !matches!(kind, Item::Directory | Item::Shortcut)
                && self.item_exists(kind, key)
            {
                lookup.record(indices, Resolution::Resolved, kind, token.loc, None);
            }
        }
    }
}
