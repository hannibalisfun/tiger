//! Compare the authored CK3 definitions in two revisions of individual script files.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::block::{BV, Block, BlockItem, Comparator, Eq, Field};
use crate::data::events::event_id;
use crate::helpers::BANNED_NAMES;
use crate::item::ItemLoader;
use crate::parse::pdxfile::{macro_syntax, parse_historical};
use crate::{Game, Item, Token};

#[derive(Deserialize)]
struct FilePair {
    path: String,
    before: Option<PathBuf>,
    after: Option<PathBuf>,
}

#[derive(PartialEq)]
enum Body {
    Value(&'static str),
    Field(&'static str, Comparator, Box<Body>),
    Block { tag: Option<&'static str>, items: Vec<Body>, macro_tokens: Option<Vec<(bool, String)>> },
}

impl Body {
    fn from_bv(bv: &BV) -> Self {
        match bv {
            BV::Block(block) => Self::from_block(block),
            BV::Value(token) => Self::Value(token.as_str()),
        }
    }

    fn from_block(block: &Block) -> Self {
        let items = block
            .iter_items()
            .map(|item| match item {
                BlockItem::Field(Field(key, cmp, value)) => {
                    Self::Field(key.as_str(), *cmp, Box::new(Self::from_bv(value)))
                }
                BlockItem::Value(value) => Self::Value(value.as_str()),
                BlockItem::Block(block) => Self::from_block(block),
            })
            .collect();
        Self::Block {
            tag: block.tag.as_ref().map(|t| t.as_str()),
            items,
            macro_tokens: block.source.as_ref().map(|source| macro_syntax(&source.0)),
        }
    }
}

struct Occurrence {
    key: Token,
    body: Body,
    priority: Option<i64>,
}

impl Occurrence {
    fn position(&self) -> Value {
        let mut pos = json!({"line": self.key.loc.line, "column": self.key.loc.column});
        if let Some(priority) = self.priority {
            pos["priority"] = priority.into();
        }
        pos
    }
}

type Definitions = BTreeMap<(&'static str, String), Vec<Occurrence>>;
type ParsedFile = (Definitions, Vec<Vec<String>>);

// These database loaders register the top-level key itself. Some other loaders
// derive identifiers from child fields, filenames or container blocks.
fn file_kind(path: &Path) -> Result<Item, String> {
    if path.extension().and_then(|x| x.to_str()) != Some("txt") {
        return Err("unsupported file format for definition comparison".into());
    }
    for kind in [
        Item::ScriptedEffect,
        Item::ScriptedTrigger,
        Item::ScriptValue,
        Item::Event,
        Item::OnAction,
        Item::Trait,
    ] {
        if path.starts_with(kind.path()) {
            return Ok(kind);
        }
    }
    let direct = [
        Item::Building,
        Item::Decision,
        Item::DecisionGroup,
        Item::DistrictType,
        Item::TradeGoodType,
        Item::TradeGoodCategory,
        Item::CharacterInteraction,
        Item::ScriptedGui,
        Item::ActivityType,
        Item::CasusBelli,
        Item::CasusBelliGroup,
    ];
    for loader in inventory::iter::<ItemLoader> {
        let kind = loader.itype();
        if loader.for_game(Game::Ck3)
            && direct.contains(&kind)
            && path.starts_with(kind.path())
            && (loader.recursive() || path.parent() == Some(Path::new(kind.path())))
        {
            return Ok(kind);
        }
    }
    Err("no supported CK3 definition loader for this path".into())
}

fn extract(path: &Path, content: String, file_kind: Item) -> Result<ParsedFile, String> {
    let (result, errors) = crate::report::inspect_historical(|| {
        let (block, directives) = parse_historical(path, content)?;
        let mut definitions = Definitions::new();
        let mut local_kind = None;
        for item in block.iter_items() {
            if file_kind == Item::Event
                && let BlockItem::Value(prefix) = item
            {
                if local_kind.is_some() {
                    return Err("file-local definition is missing its body".into());
                }
                local_kind = Some(match prefix.as_str() {
                    "scripted_effect" => Item::ScriptedEffect,
                    "scripted_trigger" => Item::ScriptedTrigger,
                    other => return Err(format!("unrecognized event file declaration `{other}`")),
                });
                continue;
            }
            let BlockItem::Field(Field(key, Comparator::Equals(Eq::Single), bv)) = item else {
                return Err("unrecognized top-level declaration or comparator".into());
            };
            let mut kind = local_kind.take().unwrap_or(file_kind);
            let mut key = key;
            let mut priority = None;
            let mut symbol = key.as_str().to_owned();
            if kind == Item::Event && key.is("namespace") {
                let BV::Value(namespace) = bv else {
                    return Err("invalid namespace declaration".into());
                };
                kind = Item::EventNamespace;
                key = namespace;
                symbol = key.as_str().to_owned();
            } else {
                if kind != Item::ScriptValue && bv.get_block().is_none() {
                    return Err(format!("expected a block for `{key}`"));
                }
                if kind == Item::Event {
                    let (ns, id) = event_id(key.as_str())
                        .ok_or_else(|| format!("invalid event ID `{key}`"))?;
                    symbol = format!("{ns}.{id}");
                    priority = Some(
                        bv.get_block()
                            .and_then(|b| b.get_field_integer("id_override_priority"))
                            .unwrap_or(0),
                    );
                }
            }
            if matches!(kind, Item::ScriptedEffect | Item::ScriptedTrigger | Item::ScriptValue)
                && BANNED_NAMES.contains(&key.as_str())
            {
                return Err(format!("reserved definition name `{key}`"));
            }
            definitions.entry((kind.into(), symbol)).or_default().push(Occurrence {
                key: key.clone(),
                body: Body::from_bv(bv),
                priority,
            });
        }
        if local_kind.is_some() {
            return Err("file-local definition is missing its body".into());
        }
        Ok((definitions, directives))
    });
    if let Some(error) = errors.first() {
        return Err(error.clone());
    }
    result
}

fn read_side(path: Option<&Path>, kind: Item) -> Result<ParsedFile, String> {
    let Some(path) = path else { return Ok(ParsedFile::default()) };
    let mut text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    if text.starts_with('\u{feff}') {
        text.drain(..3);
    }
    extract(path, text, kind)
}

// Longest common subsequence matches identical surviving occurrences even if
// an earlier duplicate was added, removed or grew by several lines.
fn changed_occurrences<'a>(
    before: &'a [Occurrence],
    after: &'a [Occurrence],
) -> Result<(Vec<&'a Occurrence>, Vec<&'a Occurrence>, bool), String> {
    let (n, m) = (before.len(), after.len());
    let cells = (n + 1).saturating_mul(m + 1);
    if cells > 1_000_000 {
        return Err("too many duplicate occurrences to match reliably".into());
    }
    let mut lengths = vec![0usize; cells];
    let index = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lengths[index(i, j)] = if before[i].body == after[j].body {
                1 + lengths[index(i + 1, j + 1)]
            } else {
                lengths[index(i + 1, j)].max(lengths[index(i, j + 1)])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let (mut old, mut new) = (Vec::new(), Vec::new());
    let mut ambiguous = false;
    while i < n || j < m {
        if i < n && j < m && before[i].body == after[j].body {
            // Identical duplicates may have different positions in the winning
            // load order. Expose ambiguous matches rather than promising an origin.
            ambiguous |= lengths[index(i, j)] == lengths[index(i + 1, j)]
                || lengths[index(i, j)] == lengths[index(i, j + 1)];
            i += 1;
            j += 1;
        } else if i < n && (j == m || lengths[index(i + 1, j)] >= lengths[index(i, j + 1)]) {
            if j < m
                && lengths[index(i + 1, j)] != 0
                && lengths[index(i + 1, j)] == lengths[index(i, j + 1)]
            {
                ambiguous = true;
            }
            old.push(&before[i]);
            i += 1;
        } else {
            new.push(&after[j]);
            j += 1;
        }
    }
    Ok((old, new, ambiguous))
}

/// Return changed definition occurrences and explicit gaps in coverage for game-relative paths.
/// The before and after blob paths are absolute. A missing side is represented by JSON null.
pub fn changed_symbols(files: &Value) -> Result<Value> {
    let pairs: Vec<FilePair> =
        serde_json::from_value(files.clone()).context("invalid changed file records")?;
    let mut changes = Vec::new();
    let mut skipped = Vec::new();
    for pair in pairs {
        let path = Path::new(&pair.path);
        ensure!(
            !path.is_absolute()
                && !pair.path.is_empty()
                && path.components().all(|p| matches!(p, std::path::Component::Normal(_))),
            "expected game-relative path"
        );
        ensure!(
            pair.before.is_some() || pair.after.is_some(),
            "no blob supplied for {}",
            pair.path
        );
        ensure!(
            pair.before.iter().chain(&pair.after).all(|p| p.is_absolute()),
            "expected absolute blob paths for {}",
            pair.path
        );
        let mut skip = |reason: String| skipped.push(json!({"path":pair.path,"reason":reason}));
        let kind = match file_kind(path) {
            Ok(kind) => kind,
            Err(error) => {
                skip(error);
                continue;
            }
        };
        let (before, old_directives) = match read_side(pair.before.as_deref(), kind) {
            Ok(defs) => defs,
            Err(error) => {
                skip(format!("before: {error}"));
                continue;
            }
        };
        let (after, new_directives) = match read_side(pair.after.as_deref(), kind) {
            Ok(defs) => defs,
            Err(error) => {
                skip(format!("after: {error}"));
                continue;
            }
        };
        if old_directives != new_directives {
            skip(
                "changed reader declarations or directives have no independent lookup symbol"
                    .to_owned(),
            );
        }
        for key in before.keys().chain(after.keys()).collect::<BTreeSet<_>>() {
            let a = before.get(key).map(Vec::as_slice).unwrap_or_default();
            let b = after.get(key).map(Vec::as_slice).unwrap_or_default();
            let (old, new, ambiguous) = match changed_occurrences(a, b) {
                Ok(diff) => diff,
                Err(error) => {
                    skip(format!("{}: {error}", key.1));
                    continue;
                }
            };
            if ambiguous {
                skip(format!("{}: duplicate occurrence matching is ambiguous", key.1));
            }
            if !new.is_empty() || !old.is_empty() {
                let mut row = json!({
                    "kind":key.0, "symbol":key.1, "path":pair.path,
                    "before":old.iter().map(|v| v.position()).collect::<Vec<_>>(),
                    "after":new.iter().map(|v| v.position()).collect::<Vec<_>>()
                });
                if ambiguous {
                    row["ambiguous"] = true.into();
                }
                changes.push(row);
            }
        }
    }
    Ok(json!({"changes":changes,"skipped":skipped}))
}
