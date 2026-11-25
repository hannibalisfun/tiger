//! CK3 symbol provenance from Tiger's loaders and layered fileset.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use strum::IntoEnumIterator;

use crate::block::Block;
use crate::ck3::data::traits::Traits;
use crate::data::events::{Events, event_id};
use crate::data::{
    on_actions::OnActions, script_values::ScriptValues, scripted_effects::Effects,
    scripted_triggers::Triggers,
};
use crate::db::Db;
use crate::fileset::{FileEntry, FileHandler};
use crate::item::ItemLoader;
use crate::{Everything, Game, Item, Loc, Token};

#[derive(Clone, Debug)]
struct Definition {
    kind: Item,
    loc: Loc,
    parameters: Vec<&'static str>,
    contribution: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct DefinitionHistory {
    query: String,
    entries: Vec<Definition>,
}

impl DefinitionHistory {
    fn new(query: &str) -> Self {
        Self { query: query.to_owned(), entries: Vec::new() }
    }

    fn push(&mut self, kind: Item, key: &Token, block: Option<&Block>, contribution: bool) {
        if matches_symbol(kind, &self.query, key.as_str()) {
            self.entries.push(Definition {
                kind,
                loc: key.loc,
                parameters: block.map(Block::macro_parms).unwrap_or_default(),
                contribution,
            });
        }
    }

    pub(crate) fn contribution(&mut self, kind: Item, key: &Token) {
        self.push(kind, key, None, true);
    }
}

pub(crate) fn matches_symbol(kind: Item, query: &str, key: &str) -> bool {
    key == query
        || (kind == Item::Event
            && event_id(query).is_some_and(|wanted| event_id(key) == Some(wanted)))
}

pub(crate) fn record(
    history: &mut Option<DefinitionHistory>,
    kind: Item,
    key: &Token,
    block: Option<&Block>,
) {
    if let Some(history) = history {
        history.push(kind, key, block, kind == Item::OnAction);
    }
}

pub(crate) fn location(data: &Everything, loc: Loc) -> Value {
    json!({
        "from": data.fileset.source_label(loc.kind),
        "path": loc.pathname().to_string_lossy().replace('\\', "/"),
        "line": loc.line,
        "column": loc.column,
    })
}

impl Everything {
    pub(crate) fn enable_definition_history(&mut self, query: &str) {
        for slot in [
            &mut self.database.lookup,
            &mut self.effects.lookup,
            &mut self.triggers.lookup,
            &mut self.script_values.lookup,
            &mut self.on_actions.lookup,
            &mut self.events.lookup,
            &mut self.traits.lookup,
        ] {
            *slot = Some(DefinitionHistory::new(query));
        }
    }

    /// Query the symbol supplied to `new_with_lookup` after loading and validation.
    pub fn lookup_result(&self, query: &str) -> Value {
        let mut winners = HashSet::new();
        for kind in Item::iter() {
            if kind == Item::Character {
                winners.extend(self.characters.primary_key(query).map(|key| (kind, key.loc)));
            } else {
                winners.extend(
                    self.iter_keys(kind)
                        .filter(|key| matches_symbol(kind, query, key.as_str()))
                        .map(|key| (kind, key.loc)),
                );
            }
        }
        let mut described = HashSet::new();
        let mut rows = BTreeMap::new();
        let mut add = |entry: &Definition, status: &str, reason: Option<&str>| {
            let mut row = location(self, entry.loc);
            row["kind"] = json!(<&'static str>::from(entry.kind));
            row["status"] = json!(status);
            row["parameters"] = json!(entry.parameters);
            if let Some(reason) = reason {
                row["reason"] = json!(reason);
            }
            if matches!(entry.kind, Item::ScriptedEffect | Item::ScriptedTrigger)
                && entry.loc.pathname().starts_with("events")
            {
                row["visibility"] = json!("file");
            }
            rows.insert(
                (
                    entry.kind,
                    entry.loc.pathname(),
                    entry.loc.kind,
                    entry.loc.line,
                    entry.loc.column,
                ),
                row,
            );
        };
        for history in [
            &self.database.lookup,
            &self.effects.lookup,
            &self.triggers.lookup,
            &self.script_values.lookup,
            &self.on_actions.lookup,
            &self.events.lookup,
            &self.traits.lookup,
        ]
        .into_iter()
        .flatten()
        {
            assert_eq!(history.query, query, "lookup query differs from the configured symbol");
            for entry in &history.entries {
                described.insert((entry.kind, entry.loc));
                let active = winners.contains(&(entry.kind, entry.loc))
                    || self.effective_file_local(entry, query);
                if entry.contribution {
                    add(entry, "contribution", None);
                } else if active {
                    add(entry, "effective", None);
                } else {
                    add(entry, "shadowed", Some("overridden_definition"));
                }
            }
        }
        // Other specialized registries expose their retained keys, even where the loader
        // doesn't keep source history for the displaced entries.
        for &(kind, loc) in winners.difference(&described) {
            add(
                &Definition { kind, loc, parameters: Vec::new(), contribution: false },
                "effective",
                None,
            );
        }
        if let Some(excluded) = &self.fileset.lookup_exclusions {
            crate::report::inspect_excluded(|| {
                let mut loaders = HashMap::<_, Vec<_>>::new();
                for loader in inventory::iter::<ItemLoader> {
                    if loader.for_game(Game::Ck3) {
                        loaders.entry(Path::new(loader.itype().path())).or_default().push(loader);
                    }
                }
                for (file, reason) in excluded {
                    for entry in self.excluded_definitions(query, file, &loaders) {
                        add(&entry, "shadowed", Some(reason));
                    }
                }
            });
        }
        let callers =
            self.lookup_callers.as_ref().map(|lookup| lookup.results(self)).unwrap_or_default();
        json!({
            "symbol": query,
            "definitions": rows.into_values().collect::<Vec<_>>(),
            "definition_coverage": "Includes displaced definitions from database item loaders, scripted effects, triggers and values, events, traits and on-actions. Other registries supply their retained keys.",
            "callers": callers,
            "caller_coverage": "Resolved references in validated script. Dynamic identifiers and unexpanded macros can have additional callers.",
        })
    }

    fn effective_file_local(&self, entry: &Definition, query: &str) -> bool {
        if !entry.loc.pathname().starts_with("events") {
            return false;
        }
        let token = Token::new(query, entry.loc);
        match entry.kind {
            Item::ScriptedEffect => self.get_effect(&token).is_some_and(|v| v.key.loc == entry.loc),
            Item::ScriptedTrigger => {
                self.get_trigger(&token).is_some_and(|v| v.key.loc == entry.loc)
            }
            _ => false,
        }
    }

    fn excluded_definitions(
        &self,
        query: &str,
        file: &FileEntry,
        loaders: &HashMap<&Path, Vec<&ItemLoader>>,
    ) -> Vec<Definition> {
        let special_kind = [
            Item::ScriptedEffect,
            Item::ScriptedTrigger,
            Item::ScriptValue,
            Item::OnAction,
            Item::Trait,
            Item::Event,
        ]
        .into_iter()
        .find(|kind| {
            file.path().starts_with(kind.path())
                && file.filename().to_string_lossy().ends_with(".txt")
        });
        let matching: Vec<_> = file
            .path()
            .ancestors()
            .filter_map(|dir| loaders.get(dir))
            .flatten()
            .copied()
            .filter(|loader| file.filename().to_string_lossy().ends_with(loader.extension()))
            .collect();
        if special_kind.is_none() && matching.is_empty() {
            return Vec::new();
        }
        // Inactive files aren't part of the validated parse tree. Only parse a candidate
        // when its text or whole-file key can contain this exact symbol.
        let whole_file_match = matching.iter().any(|l| {
            l.whole_file()
                && file.filename().to_string_lossy().strip_suffix(l.extension()) == Some(query)
        });
        // An event file may spell the same numeric ID with different leading zeroes.
        let needle = if special_kind == Some(Item::Event) {
            event_id(query).map_or(query, |(namespace, _)| namespace)
        } else {
            query
        };
        let could_define = |bytes: Vec<u8>| {
            // Reader directives and calculations can build keys absent from the literal text.
            // Detect-encoding loaders may also accept Windows-1252 names.
            bytes.contains(&b'@')
                || (!query.is_ascii() && !bytes.is_ascii())
                || String::from_utf8_lossy(&bytes).contains(needle)
        };
        if !whole_file_match && !fs::read(file.fullpath()).is_ok_and(could_define) {
            return Vec::new();
        }
        let mut history = DefinitionHistory::new(query);
        // Use the native handlers in isolation. They pick the actual key, including
        // file-local event scripts, scalar script values and reserved-name exclusions.
        macro_rules! inspect_handler {
            ($handler:ty) => {{
                let mut handler = <$handler>::default();
                handler.lookup = Some(history);
                if let Some(block) = handler.load_file(file, &self.parser) {
                    handler.handle_file(file, block);
                }
                history = handler.lookup.unwrap();
            }};
        }
        match special_kind {
            Some(Item::Event) => inspect_handler!(Events),
            Some(Item::OnAction) => inspect_handler!(OnActions),
            Some(Item::Trait) => inspect_handler!(Traits),
            Some(Item::ScriptValue) => inspect_handler!(ScriptValues),
            Some(Item::ScriptedEffect) => inspect_handler!(Effects),
            Some(Item::ScriptedTrigger) => inspect_handler!(Triggers),
            _ => (),
        }
        if !matching.is_empty() {
            let mut db = Db::default();
            db.lookup = Some(history);
            for loader in matching {
                let depth = Path::new(loader.itype().path()).components().count() + 1;
                if let Some(block) = loader.read_file(file, depth, &self.parser) {
                    loader.add_block(&mut db, block);
                }
            }
            db.add_subitems();
            history = db.lookup.unwrap();
        }
        history.entries
    }
}
