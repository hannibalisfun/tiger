use crate::block::Block;
use crate::ck3::tables::targets::{scope_prefix, scope_to_scope};
use crate::context::ScopeContext;
use crate::everything::Everything;
use crate::game::Game;
use crate::helpers::TigerHashMap;
use crate::report::{ErrorKey, Severity, err};
use crate::scopes::Scopes;
use crate::token::Token;
use crate::tooltipped::Tooltipped;
use crate::trigger::{PartFlags, validate_inscopes, validate_target_ok_this};
use crate::validate::{validate_identifier, validate_optional_duration};
use crate::validator::{Validator, ValueValidator};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Lifetime {
    Scoped,
    Global,
    Local,
}

impl Lifetime {
    fn of(api: &str) -> Self {
        if api.contains("global_variable_map") {
            Self::Global
        } else if api.contains("local_variable_map") {
            Self::Local
        } else {
            Self::Scoped
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct MapTypes {
    keys: Scopes,
    values: Scopes,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) struct LocalMap {
    types: Option<MapTypes>,
    cleared: bool,
}

impl LocalMap {
    pub fn apply(&mut self, change: Self) {
        if change.cleared || self.types.is_none() {
            *self = Self { cleared: self.cleared || change.cleared, ..change };
        } else if let (Some(existing), Some(added)) = (&mut self.types, change.types) {
            existing.include(added);
        }
    }
}

impl MapTypes {
    fn unknown() -> Self {
        Self { keys: Scopes::all_but_none(), values: Scopes::all_but_none() }
    }

    // Entries can have different types. An unresolved write leaves that side unconstrained.
    fn include(&mut self, other: Self) {
        self.keys |= other.keys;
        self.values |= other.values;
    }
}

#[derive(Debug, Default)]
pub(crate) struct PersistentMaps(TigerHashMap<(Lifetime, &'static str), MapTypes>);

impl PersistentMaps {
    pub fn scan_write(&mut self, api: &str, block: &Block) {
        let Some(name) = block.get_field_value("name") else { return };
        let types = MapTypes {
            keys: literal_type(block.get_field_value("key")),
            values: literal_type(block.get_field_value("value")),
        };
        self.0
            .entry((Lifetime::of(api), name.as_str()))
            .and_modify(|s| s.include(types))
            .or_insert(types);
    }
}

fn literal_type(token: Option<&Token>) -> Scopes {
    let Some(token) = token else { return Scopes::all_but_none() };
    if token.is_number() {
        return Scopes::Value;
    }
    let text = token.as_str();
    if text.contains(['$', '(', ')']) {
        return Scopes::all_but_none();
    }
    let last = text.rsplit('.').next().unwrap().to_ascii_lowercase();
    let result = if let Some((prefix, _)) = last.split_once(':') {
        scope_prefix(prefix).map(|(_, out, _)| out)
    } else {
        scope_to_scope(&last).map(|(_, out)| out)
    };
    result.unwrap_or(Scopes::all_but_none()).difference(Scopes::None)
}

fn map_types(api: &str, name: &Token, data: &Everything, sc: &ScopeContext) -> MapTypes {
    let lifetime = Lifetime::of(api);
    if lifetime == Lifetime::Local {
        sc.local_maps.get(name.as_str()).and_then(|map| map.types)
    } else {
        data.variables.maps.0.get(&(lifetime, name.as_str())).copied()
    }
    .unwrap_or_else(MapTypes::unknown)
}

pub fn validate_mutation(
    api: &Token,
    _block: &Block,
    data: &Everything,
    sc: &mut ScopeContext,
    mut vd: Validator,
    _tooltipped: Tooltipped,
) {
    vd.req_field("name");
    vd.req_field("key");
    let name = vd.field_identifier("name", "map name").cloned();
    let key = vd.field_value("key").cloned();
    let api = api.as_str().to_ascii_lowercase();
    let adding = api.starts_with("add_");
    let known =
        name.as_ref().map_or_else(MapTypes::unknown, |n| map_types(api.as_str(), n, data, sc));
    let keys = key.as_ref().map_or(Scopes::all_but_none(), |k| {
        validate_target_ok_this(
            k,
            data,
            sc,
            if adding { Scopes::all_but_none() } else { known.keys },
        )
    });
    if adding {
        vd.req_field("value");
        if let Some(value) = vd.field_value("value") {
            let values = validate_target_ok_this(value, data, sc, Scopes::all_but_none());
            if Lifetime::of(api.as_str()) == Lifetime::Local
                && let Some(name) = name
            {
                let types = MapTypes { keys, values };
                sc.local_maps
                    .entry(name.as_str())
                    .or_default()
                    .apply(LocalMap { types: Some(types), cleared: false });
            }
        }
        validate_optional_duration(&mut vd, sc);
    }
}

pub fn clear_local(_api: &Token, mut vd: ValueValidator, sc: &mut ScopeContext, _tip: Tooltipped) {
    vd.identifier("map name");
    sc.local_maps.insert(vd.value().as_str(), LocalMap { types: None, cleared: true });
}

fn lookup_args(text: &str) -> Option<&str> {
    let (left, rest) = text.split_once('(')?;
    let (args, tail) = rest.split_once(')')?;
    if left.chars().any(char::is_whitespace) || !(tail.is_empty() || tail.starts_with('.')) {
        return None;
    }
    let field = left.rsplit('.').next()?;
    ["variable_map", "local_variable_map", "global_variable_map"]
        .iter()
        .any(|name| field.eq_ignore_ascii_case(name))
        .then_some(args)
}

pub fn check_quoted_parameter(token: &Token) {
    let text = token.as_str();
    if text.contains('$')
        && let Some(args) = lookup_args(text)
        && let Some((_, tail)) = args.split_once('$')
        && let Some((name, _)) = tail.split_once('$')
        && !name.is_empty()
    {
        err(ErrorKey::Validation)
            .msg("macro parameter in a quoted variable map lookup causes a CK3 load error")
            .info("Save the argument as a scope value and use the saved scope in the lookup.")
            .loc(token)
            .push();
    }
}

pub fn invalid_macro_arguments(args: &[(&str, Token)]) -> bool {
    if !Game::is_ck3() {
        return false;
    }
    let mut invalid = false;
    for (_, token) in args {
        if lookup_args(token.as_str()).is_some() {
            err(ErrorKey::Macro)
                .msg("a variable map lookup loses its quotes when passed as a macro argument")
                .info("Save the lookup result as a scope value and pass the saved scope.")
                .loc(token)
                .push();
            invalid = true;
        }
    }
    invalid
}

pub fn validate_membership(
    api: &Token,
    mut vd: Validator,
    data: &Everything,
    sc: &mut ScopeContext,
) {
    let api = api.as_str().to_ascii_lowercase();
    vd.req_field("name");
    vd.req_field("target");
    let types = vd
        .field_identifier("name", "map name")
        .map_or_else(MapTypes::unknown, |name| map_types(api.as_str(), name, data, sc));
    if let Some(target) = vd.field_value("target") {
        let expected = if api.starts_with("is_key_") { types.keys } else { types.values };
        validate_target_ok_this(target, data, sc, expected);
    }
}

pub fn iterator_scope(
    api: &str,
    block: &Block,
    data: &Everything,
    sc: &ScopeContext,
) -> Option<Scopes> {
    if !api.starts_with("key_in_") || !api.ends_with("variable_map") || !Game::is_ck3() {
        return None;
    }
    block.get_field_value("variable").map(|name| map_types(api, name, data, sc).keys)
}

pub fn lookup(
    flags: PartFlags,
    func: &Token,
    arg: &Token,
    data: &Everything,
    sc: &mut ScopeContext,
) {
    let api = func.as_str().to_ascii_lowercase();
    let input = if Lifetime::of(&api) == Lifetime::Scoped {
        crate::ck3::scopes::VARIABLE_STORAGE
    } else {
        Scopes::None
    };
    validate_inscopes(flags, func, input, sc, data);
    let Some((name, key)) = arg.split_once('|').filter(|(n, k)| {
        !n.as_str().is_empty() && !k.as_str().is_empty() && !k.as_str().contains('|')
    }) else {
        err(ErrorKey::Validation)
            .msg("expected map name and key separated by a single `|`")
            .loc(arg)
            .push();
        sc.replace(Scopes::all_but_none(), func.clone());
        return;
    };
    validate_identifier(&name, "map name", Severity::Error);
    let types = map_types(&api, &name, data, sc);
    let receiver = sc.scopes(data);
    let stash = sc.stash_builder();
    // In owner.variable_map(name|this), this is the owner. An unchained read keeps its caller's prev.
    if !flags.contains(PartFlags::First) {
        sc.open_scope(receiver, func.clone());
    }
    if !key.as_str().contains('$') {
        validate_target_ok_this(&key, data, sc, types.keys);
    }
    if !flags.contains(PartFlags::First) {
        sc.close();
    }
    sc.unstash_builder(stash);
    sc.replace(types.values, func.clone());
}
