//! Static firing paths for an active CK3 on-action in an exact symbol lookup.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{LazyLock, Mutex};

use serde_json::{Value, json};

use crate::lookup::location;
use crate::lookup_callers::{Owners, expansion_chain};
use crate::{Everything, Item, Loc, Token};

// Sorted names marked "From Code: Yes" in the CK3 1.21.0 on_actions.log export.
static ENGINE_HOOKS: LazyLock<BTreeSet<&str>> =
    LazyLock::new(|| include_str!("ck3/tables/engine_on_actions.txt").lines().collect());

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct FiringSite {
    target: &'static str,
    via: &'static str,
    // Registrations have a known parent. An explicit dispatch can occur in any effect context.
    parent: Option<&'static str>,
    loc: Loc,
}

impl FiringSite {
    fn owner(self, owners: &Owners) -> Option<(Item, &'static str)> {
        self.parent
            .map(|name| (Item::OnAction, name))
            .or_else(|| expansion_chain(self.loc).last().and_then(|site| owners.at(site)))
    }

    fn step(self, data: &Everything, owners: &Owners) -> Value {
        let owner = self.owner(owners);
        json!({
            "caller": owner.map(|(_, name)| name),
            "caller_kind": owner.map(|(kind, _)| <&'static str>::from(kind)),
            "via": self.via,
            "on_action": self.target,
            "source": location(data, self.loc),
            "expansion": expansion_chain(self.loc).skip(1).map(|loc| location(data, loc)).collect::<Vec<_>>(),
        })
    }
}

#[derive(Debug, Default)]
pub(crate) struct FiringSites(Mutex<BTreeSet<FiringSite>>);

impl Everything {
    pub(crate) fn record_registration(
        &self,
        parent: Option<&Token>,
        via: &'static str,
        target: &Token,
    ) {
        if let Some(parent) = parent {
            self.record_firing(FiringSite {
                target: target.as_str(),
                via,
                parent: Some(parent.as_str()),
                loc: target.loc,
            });
        }
    }

    pub(crate) fn record_trigger_event(&self, effect: &Token, target: &Token) {
        self.record_firing(FiringSite {
            target: target.as_str(),
            via: "trigger_event",
            parent: None,
            loc: effect.loc,
        });
    }

    fn record_firing(&self, site: FiringSite) {
        if let Some(sites) = self.lookup_callers.as_ref().and_then(|lookup| lookup.firings.as_ref())
        {
            sites.0.lock().unwrap().insert(site);
        }
    }
}

impl FiringSites {
    pub(crate) fn reachability(&self, data: &Everything, query: &str) -> Value {
        let sites = self.0.lock().unwrap();
        let owners =
            Owners::for_sites(data, sites.iter().filter(|s| s.parent.is_none()).map(|s| s.loc));
        let mut incoming = BTreeMap::<_, Vec<_>>::new();
        for &site in sites.iter() {
            incoming.entry(site.target).or_default().push(site);
        }
        // Stable source order breaks ties between equally short paths across parallel runs.
        for edges in incoming.values_mut() {
            edges.sort_by_cached_key(|edge| {
                let locations = expansion_chain(edge.loc)
                    .map(|s| (s.pathname(), s.kind, s.line, s.column))
                    .collect::<Vec<_>>();
                (edge.owner(&owners), edge.via, locations)
            });
        }

        // Walk back from the requested hook. Each visited parent keeps one shortest
        // route to the query, including when registrations form a cycle.
        let mut next = BTreeMap::from([(query, None)]);
        let mut queue = VecDeque::from([query]);
        let mut engine_paths = Vec::new();
        let mut script_calls = Vec::new();
        let mut external_call = false;
        while let Some(name) = queue.pop_front() {
            let edges = incoming.get(name).map(Vec::as_slice).unwrap_or_default();
            let engine_root = ENGINE_HOOKS.contains(name);
            let path = if engine_root || edges.iter().any(|s| s.parent.is_none()) {
                std::iter::successors(next[name], |edge: &FiringSite| next[edge.target])
                    .map(|edge| edge.step(data, &owners))
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            if engine_root {
                engine_paths.push(json!({"root": name, "steps": path}));
            }
            for &edge in edges {
                let owner = edge.owner(&owners);
                if edge.parent.is_none() {
                    let steps: Vec<_> = std::iter::once(edge.step(data, &owners))
                        .chain(path.iter().cloned())
                        .collect();
                    script_calls.push(json!({"steps": steps}));
                    external_call |= !matches!(owner, Some((Item::OnAction, _)));
                }
                if let Some((Item::OnAction, parent)) = owner
                    && let std::collections::btree_map::Entry::Vacant(entry) = next.entry(parent)
                {
                    entry.insert(Some(edge));
                    queue.push_back(parent);
                }
            }
        }
        let status = if !engine_paths.is_empty() {
            "engine path"
        } else if external_call {
            "script caller"
        } else {
            "no known firing path"
        };
        json!({"status": status, "engine_paths": engine_paths, "script_calls": script_calls})
    }
}
