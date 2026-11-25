//! CK3 symbol provenance from Tiger's loaders and layered fileset.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aho_corasick::AhoCorasick;
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
use crate::lookup_callers::CallerResults;
use crate::pdxfile::{PdxEncoding, PdxFile};
use crate::{Everything, Game, Item, Loc, Token};

#[derive(Clone, Debug)]
struct Definition {
    kind: Item,
    name: &'static str,
    loc: Loc,
    parameters: Vec<&'static str>,
    contribution: bool,
}

/// Each distinct query has one result slot. Event aliases can share a native
/// identity while continuing to match unrelated item kinds by exact spelling.
#[derive(Debug)]
pub(crate) struct LookupQueries {
    exact: HashMap<String, usize>,
    event_aliases: HashMap<String, HashMap<u16, Vec<usize>>>,
    literals: Option<AhoCorasick>,
    namespaces: Option<AhoCorasick>,
    all_ascii: bool,
}

impl LookupQueries {
    pub(crate) fn new(names: &[String]) -> Self {
        let mut exact = HashMap::new();
        let mut event_aliases = HashMap::<String, HashMap<u16, Vec<usize>>>::new();
        for name in names {
            let index = exact.len();
            if let std::collections::hash_map::Entry::Vacant(slot) = exact.entry(name.clone()) {
                slot.insert(index);
                if let Some((namespace, number)) = event_id(name) {
                    event_aliases
                        .entry(namespace.to_owned())
                        .or_default()
                        .entry(number)
                        .or_default()
                        .push(index);
                }
            }
        }
        // If an exceptionally large pattern set exceeds the automaton's size limit,
        // the native loader still sees all potentially relevant files.
        let literals = AhoCorasick::new(exact.keys()).ok();
        let namespaces = AhoCorasick::new(event_aliases.keys()).ok();
        let all_ascii = names.iter().all(|s| s.is_ascii());
        Self { exact, event_aliases, literals, namespaces, all_ascii }
    }

    pub(crate) fn len(&self) -> usize {
        self.exact.len()
    }

    pub(crate) fn indices(&self, kind: Item, name: &str) -> &[usize] {
        if kind == Item::Event
            && let Some((namespace, id)) = event_id(name)
        {
            return self
                .event_aliases
                .get(namespace)
                .and_then(|events| events.get(&id))
                .map_or(&[], Vec::as_slice);
        }
        self.exact.get(name).map_or(&[], std::slice::from_ref)
    }

    fn could_define(&self, bytes: &[u8], event_file: bool) -> bool {
        // Reader directives, calculations and Windows-1252 names can differ from their literal spelling.
        bytes.contains(&b'@')
            || (!self.all_ascii && !bytes.is_ascii())
            || self.literals.as_ref().is_none_or(|m| m.is_match(bytes))
            || (event_file && self.namespaces.as_ref().is_none_or(|m| m.is_match(bytes)))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DefinitionHistory {
    queries: Arc<LookupQueries>,
    entries: Vec<Definition>,
}

impl DefinitionHistory {
    fn new(queries: &Arc<LookupQueries>) -> Self {
        Self { queries: Arc::clone(queries), entries: Vec::new() }
    }

    fn push(&mut self, kind: Item, key: &Token, block: Option<&Block>, contribution: bool) {
        if !self.queries.indices(kind, key.as_str()).is_empty() {
            self.entries.push(Definition {
                kind,
                name: key.as_str(),
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
    /// Identify a higher layer that masks this path even if its lower copy is absent.
    pub fn lookup_file_mask(&self, from: &str, path: &Path) -> Option<Value> {
        self.fileset
            .lookup_file_mask(from, path)
            .map(|(reason, from)| json!({ "reason": reason, "from": from }))
    }

    /// The current lower-layer copy that can surface when an exact-path overlay is removed.
    pub fn lookup_lower_file(&self, from: &str, path: &Path) -> Option<(String, PathBuf)> {
        self.fileset.lookup_lower_file(from, path).map(|entry| {
            (self.fileset.source_label(entry.kind()).to_owned(), entry.fullpath().to_owned())
        })
    }

    /// Select symbols after constructing the lookup fileset and before loading script.
    pub fn set_lookup_queries(&mut self, queries: &[String]) {
        let queries = Arc::new(LookupQueries::new(queries));
        self.lookup_callers = Some(crate::lookup_callers::CallerLookup::new(Arc::clone(&queries)));
        for slot in [
            &mut self.database.lookup,
            &mut self.effects.lookup,
            &mut self.triggers.lookup,
            &mut self.script_values.lookup,
            &mut self.on_actions.lookup,
            &mut self.events.lookup,
            &mut self.traits.lookup,
        ] {
            *slot = Some(DefinitionHistory::new(&queries));
        }
    }

    /// Query the symbol supplied to `new_with_lookup` after loading and validation.
    pub fn lookup_result(&self, query: &str) -> Value {
        self.lookup_results(&[query.to_owned()])[0].take()
    }

    /// Return one lookup object per requested spelling, preserving order and duplicates.
    pub fn lookup_results(&self, requested: &[String]) -> Value {
        if requested.is_empty() {
            return json!([]);
        }
        let queries = self
            .lookup_callers
            .as_ref()
            .map(|lookup| Arc::clone(&lookup.queries))
            .unwrap_or_else(|| Arc::new(LookupQueries::new(requested)));
        let mut winners = HashSet::new();
        for kind in Item::iter() {
            if kind == Item::Character {
                for name in queries.exact.keys() {
                    if let Some(key) = self.characters.primary_key(name) {
                        winners.insert((kind, key.loc, key.as_str()));
                    }
                }
            } else {
                winners.extend(
                    self.iter_keys(kind)
                        .filter(|key| !queries.indices(kind, key.as_str()).is_empty())
                        .map(|key| (kind, key.loc, key.as_str())),
                );
            }
        }
        let mut described = HashSet::new();
        let mut rows = vec![BTreeMap::new(); queries.len()];
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
            let order = (
                entry.kind,
                entry.loc.pathname(),
                entry.loc.kind,
                entry.loc.line,
                entry.loc.column,
            );
            for &index in queries.indices(entry.kind, entry.name) {
                rows[index].insert(order, row.clone());
            }
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
            for entry in &history.entries {
                let definition = (entry.kind, entry.loc, entry.name);
                described.insert(definition);
                let active = winners.contains(&definition) || self.effective_file_local(entry);
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
        for &(kind, loc, name) in winners.difference(&described) {
            add(
                &Definition { kind, name, loc, parameters: Vec::new(), contribution: false },
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
                    for entry in self.excluded_definitions(&queries, file, &loaders) {
                        add(&entry, "shadowed", Some(reason));
                    }
                }
            });
        }
        let callers = self
            .lookup_callers
            .as_ref()
            .map(|lookup| lookup.results(self))
            .unwrap_or_else(|| vec![CallerResults::default(); queries.len()]);
        let include_unresolved =
            self.lookup_callers.as_ref().is_some_and(|lookup| lookup.include_unresolved);
        let mut found: Vec<Value> = rows.into_iter().zip(callers).map(|(definitions, callers)| {
            let mut result = json!({
            "definitions": definitions.into_values().collect::<Vec<_>>(),
            "definition_coverage": "Includes displaced definitions from database item loaders, scripted effects, triggers and values, events, traits and on-actions. Other registries supply their retained keys.",
            "callers": callers.resolved,
            "caller_coverage": "Resolved references in validated script. Dynamic identifiers and unexpanded macros can have additional callers.",
            });
            if include_unresolved {
                result["unresolved_callers"] = Value::Array(callers.unresolved);
            }
            result
        }).collect();
        for (name, &index) in &queries.exact {
            found[index]["symbol"] = json!(name);
        }
        if requested.len() == found.len()
            && requested.iter().enumerate().all(|(i, name)| queries.exact.get(name) == Some(&i))
        {
            Value::Array(found)
        } else {
            Value::Array(requested.iter().map(|name| found[queries.exact[name]].clone()).collect())
        }
    }

    fn effective_file_local(&self, entry: &Definition) -> bool {
        if !entry.loc.pathname().starts_with("events") {
            return false;
        }
        let token = Token::new(entry.name, entry.loc);
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
        queries: &Arc<LookupQueries>,
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
            .filter(|loader| {
                loader.accepts_file(file, Path::new(loader.itype().path()).components().count() + 1)
            })
            .collect();
        if special_kind.is_none() && matching.is_empty() {
            return Vec::new();
        }
        // Inactive files aren't part of the validated parse tree. Only parse a candidate
        // when its text or whole-file key can contain this exact symbol.
        let whole_file_match = matching.iter().any(|l| {
            l.whole_file()
                && file
                    .filename()
                    .to_string_lossy()
                    .strip_suffix(l.extension())
                    .is_some_and(|name| queries.exact.contains_key(name))
        });
        if !whole_file_match
            && !fs::read(file.fullpath())
                .is_ok_and(|bytes| queries.could_define(&bytes, special_kind == Some(Item::Event)))
        {
            return Vec::new();
        }
        let mut db = (!matching.is_empty()).then(|| {
            let mut db = Db::default();
            db.lookup = Some(DefinitionHistory::new(queries));
            db
        });
        let mut by_encoding = HashMap::<_, Vec<_>>::new();
        if special_kind.is_some() {
            by_encoding.entry(PdxEncoding::Utf8Bom).or_default();
        }
        for loader in matching {
            by_encoding.entry(loader.encoding()).or_default().push(loader);
        }
        let mut entries = Vec::new();
        for (encoding, loaders) in by_encoding {
            let Some(block) = PdxFile::read_encoded(file, encoding, &self.parser) else { continue };
            // CK3's special script handlers all use UTF-8 with BOM. When loaders overlap,
            // reuse the parse for every consumer with the same encoding.
            if let Some(kind) = special_kind
                && encoding == PdxEncoding::Utf8Bom
            {
                if loaders.is_empty() {
                    entries.extend(excluded_script_definitions(queries, file, kind, block));
                    continue;
                }
                entries.extend(excluded_script_definitions(queries, file, kind, block.clone()));
            }
            if let Some((last, others)) = loaders.split_last()
                && let Some(db) = db.as_mut()
            {
                for loader in others {
                    loader.add_block(db, block.clone());
                }
                last.add_block(db, block);
            }
        }
        if let Some(mut db) = db {
            db.add_subitems();
            entries.extend(db.lookup.unwrap().entries);
        }
        entries
    }
}

fn excluded_script_definitions(
    queries: &Arc<LookupQueries>,
    file: &FileEntry,
    kind: Item,
    block: Block,
) -> Vec<Definition> {
    // The native handlers pick the actual key, including file-local events and scalar script values.
    macro_rules! inspect_handler {
        ($handler:ty) => {{
            let mut handler = <$handler>::default();
            handler.lookup = Some(DefinitionHistory::new(queries));
            handler.handle_file(file, block);
            handler.lookup.unwrap().entries
        }};
    }
    match kind {
        Item::Event => inspect_handler!(Events),
        Item::OnAction => inspect_handler!(OnActions),
        Item::Trait => inspect_handler!(Traits),
        Item::ScriptValue => inspect_handler!(ScriptValues),
        Item::ScriptedEffect => inspect_handler!(Effects),
        Item::ScriptedTrigger => inspect_handler!(Triggers),
        _ => unreachable!(),
    }
}
