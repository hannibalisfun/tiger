//! References resolved by the CK3 validators during an exact symbol lookup.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Mutex;

use serde_json::Value;

use crate::everything::Everything;
use crate::item::Item;
use crate::lookup::{location, matches_symbol};
use crate::macros::MACRO_MAP;
use crate::pathtable::PathTableIndex;
use crate::token::{Loc, Token};

#[derive(Debug)]
pub(crate) struct CallerLookup {
    query: String,
    hits: Mutex<BTreeMap<(Item, Loc), Option<Loc>>>,
}

impl CallerLookup {
    pub(crate) fn new(query: &str) -> Self {
        Self { query: query.to_owned(), hits: Mutex::default() }
    }

    fn record(&self, kind: Item, loc: Loc, target: Option<Loc>) {
        self.hits
            .lock()
            .unwrap()
            .entry((kind, loc))
            .and_modify(|existing| {
                if target.is_some() {
                    *existing = target;
                }
            })
            .or_insert(target);
    }

    pub(crate) fn results(&self, data: &Everything) -> Vec<Value> {
        let hits = self.hits.lock().unwrap();
        let mut owners = Owners::new();
        let mut paths = BTreeSet::new();
        for (_, loc) in hits.keys() {
            for site in expansion_chain(*loc) {
                owners.entry(site.idx).or_default();
                paths.insert(site.pathname());
            }
        }
        if owners.is_empty() {
            return Vec::new();
        }
        // Only these registries represent top-level definitions throughout their files.
        // In particular, a trait flag or a variable named by a nested field is not an owner.
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
                    add_owner(&mut owners, kind, key);
                }
            }
        }
        for (kind, key) in data.events.local_script_keys() {
            add_owner(&mut owners, kind, key);
        }
        for key in data.on_actions.fragment_keys() {
            add_owner(&mut owners, Item::OnAction, key);
        }
        for keys in owners.values_mut() {
            keys.sort_unstable_by_key(|(loc, _, _)| (loc.line, loc.column));
        }

        // Path-table indices depend on file registration order. Sort by public locations
        // (including the invocation chain) for identical results across worker counts.
        let mut rows = BTreeMap::new();
        for (&(kind, loc), target) in hits.iter() {
            let mut sort_key = Vec::new();
            let mut sites = expansion_chain(loc).map(|site| {
                sort_key.push((site.pathname(), site.kind, site.line, site.column));
                let mut value = location(data, site);
                if let Some(keys) = owners.get(&site.idx) {
                    let index = keys.partition_point(|(start, _, _)| {
                        (start.line, start.column) <= (site.line, site.column)
                    });
                    if index > 0 {
                        let (_, kind, name) = keys[index - 1];
                        value["caller"] = name.into();
                        value["caller_kind"] = <&'static str>::from(kind).into();
                    }
                }
                value
            });
            let mut row = sites.next().unwrap();
            row["kind"] = <&'static str>::from(kind).into();
            if let Some(target) = target {
                row["target"] = location(data, *target);
            }
            row["expansion"] = sites.collect::<Vec<_>>().into();
            rows.insert((sort_key, kind), row);
        }
        rows.into_values().collect()
    }
}

type Owners = BTreeMap<PathTableIndex, Vec<(Loc, Item, &'static str)>>;

fn add_owner(owners: &mut Owners, kind: Item, key: &Token) {
    if let Some(keys) = owners.get_mut(&key.loc.idx) {
        keys.push((key.loc, kind, key.as_str()));
    }
}

fn expansion_chain(loc: Loc) -> impl Iterator<Item = Loc> {
    std::iter::successors(Some(loc), |site| site.link_idx.and_then(|idx| MACRO_MAP.get_loc(idx)))
}

impl Everything {
    pub(crate) fn record_caller(&self, kind: Item, token: &Token, target: Option<&Token>) {
        if let Some(lookup) = &self.lookup_callers
            && matches_symbol(kind, &lookup.query, token.as_str())
        {
            lookup.record(kind, token.loc, target.map(|key| key.loc));
        }
    }

    pub(crate) fn record_typed_caller(&self, kind: Item, key: &str, token: &Token) {
        if let Some(lookup) = &self.lookup_callers
            && matches_symbol(kind, &lookup.query, key)
            && !matches!(kind, Item::Directory | Item::Shortcut)
            && self.item_exists(kind, key)
        {
            lookup.record(kind, token.loc, None);
        }
    }
}
