use std::path::PathBuf;

use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use crate::block::Block;
use crate::context::ScopeContext;
use crate::effect::validate_effect;
use crate::everything::Everything;
use crate::fileset::{FileEntry, FileHandler};
use crate::game::Game;
use crate::helpers::TigerHashMap;
use crate::item::Item;
use crate::on_action::on_action_scopecontext;
use crate::parse::ParserMemory;
use crate::pdxfile::PdxFile;
#[cfg(any(feature = "ck3", feature = "hoi4"))]
use crate::report::err;
use crate::report::{ErrorKey, warn};
use crate::scopes::Scopes;
use crate::token::Token;
use crate::tooltipped::Tooltipped;
use crate::trigger::validate_trigger;
#[cfg(feature = "jomini")]
use crate::validate::validate_duration;
use crate::validate::validate_modifiers_with_base;
use crate::validator::Validator;
use crate::variables::Variables;

#[derive(Clone, Debug, Default)]
pub struct OnActions {
    #[cfg(feature = "ck3")]
    pub(crate) lookup: Option<crate::lookup::DefinitionHistory>,
    on_actions: TigerHashMap<&'static str, OnAction>,
}

impl OnActions {
    fn load_item(&mut self, key: Token, block: Block) {
        #[cfg(feature = "ck3")]
        crate::lookup::record(&mut self.lookup, Item::OnAction, &key, None);
        if let Some(other) = self.on_actions.get_mut(key.as_str()) {
            other.add(key, block);
        } else {
            self.on_actions.insert(key.as_str(), OnAction::new(key, block));
        }
    }

    pub fn scan_variables(&self, registry: &mut Variables) {
        for item in self.on_actions.values() {
            for (_, block) in &item.actions {
                registry.scan(block);
            }
        }
    }

    pub fn exists(&self, key: &str) -> bool {
        self.on_actions.contains_key(key)
    }

    pub fn iter_keys(&self) -> impl Iterator<Item = &Token> {
        // SAFETY: The item's constructor guarantees at least one element in `actions`.
        self.on_actions.values().map(|item| &item.actions[0].0)
    }

    pub fn validate(&self, data: &Everything) {
        self.on_actions.par_iter().for_each(|(_, v)| {
            v.validate(data);
        });
    }

    #[cfg(feature = "ck3")]
    pub(crate) fn fragment_keys(&self) -> impl Iterator<Item = &Token> {
        self.on_actions.values().flat_map(|action| action.actions.iter().map(|(key, _)| key))
    }

    pub fn validate_call(&self, key: &Token, data: &Everything, sc: &mut ScopeContext) {
        if let Some(action) = self.on_actions.get(key.as_str()) {
            #[cfg(feature = "ck3")]
            data.record_caller(Item::OnAction, key, None);
            action.validate_call(data, sc);
        }
    }
}

impl FileHandler<Block> for OnActions {
    fn subpath(&self) -> PathBuf {
        match Game::game() {
            #[cfg(feature = "ck3")]
            Game::Ck3 => PathBuf::from("common/on_action"),
            #[cfg(feature = "vic3")]
            Game::Vic3 => PathBuf::from("common/on_actions"),
            #[cfg(feature = "imperator")]
            Game::Imperator => PathBuf::from("common/on_action"),
            #[cfg(feature = "eu5")]
            Game::Eu5 => PathBuf::from("common/on_action"),
            #[cfg(feature = "hoi4")]
            Game::Hoi4 => PathBuf::from("common/on_actions"),
        }
    }

    fn load_file(&self, entry: &FileEntry, parser: &ParserMemory) -> Option<Block> {
        if !entry.filename().to_string_lossy().ends_with(".txt") {
            return None;
        }

        #[cfg(feature = "hoi4")]
        if Game::is_hoi4() {
            return PdxFile::read_no_bom(entry, parser);
        }
        PdxFile::read(entry, parser)
    }

    fn handle_file(&mut self, _entry: &FileEntry, mut block: Block) {
        for (key, block) in block.drain_definitions_warn() {
            if Game::is_hoi4() {
                #[cfg(feature = "hoi4")]
                let mut block = block;
                #[cfg(feature = "hoi4")]
                if key.is("on_actions") {
                    for (key, block) in block.drain_definitions_warn() {
                        self.load_item(key, block);
                    }
                } else {
                    let msg = "unexpected key";
                    let info = "expected only `on_actions` here";
                    err(ErrorKey::UnknownField).msg(msg).info(info).loc(key).push();
                }
            } else {
                self.load_item(key, block);
            }
        }
    }
}

#[derive(Clone, Debug)]
/// Actions override in a special way, which is why this struct contains all actions defined under
/// the same name, rather than each new one replacing the prior one.
pub struct OnAction {
    actions: Vec<(Token, Block)>,
}

impl OnAction {
    pub fn new(key: Token, block: Block) -> Self {
        Self { actions: vec![(key, block)] }
    }

    pub fn add(&mut self, key: Token, block: Block) {
        self.actions.push((key, block));
    }

    pub fn validate(&self, data: &Everything) {
        let mut winners = OnActionBlocks::default();
        for (key, block) in self.actions.iter().rev() {
            // Make an sc for each array entry, to make sure it uses the local `key`.
            // This is important to distinguish between vanilla errors and mod errors.
            let mut sc = if let Some(builtin_sc) = on_action_scopecontext(key, data) {
                builtin_sc
            } else {
                let mut generated_sc = ScopeContext::new(Scopes::non_primitive(), key);
                generated_sc.set_strict_scopes(false);
                generated_sc
            };
            validate_on_action_internal(block, data, &mut sc, &mut winners, Some(key));
        }
    }

    /// Revalidate an `on_action` under the given scope context.
    /// It is not necessary to validate anything non scope related,
    /// but in this case it is easier to just call the full function because
    /// everything in an action is scope related.
    pub fn validate_call(&self, data: &Everything, sc: &mut ScopeContext) {
        let mut winners = OnActionBlocks::default();
        for (_, block) in self.actions.iter().rev() {
            validate_on_action_internal(block, data, sc, &mut winners, None);
        }
    }
}

#[derive(Default)]
struct OnActionBlocks {
    trigger: Option<Token>,
    effect: Option<Token>,
}

fn validate_single_block(
    vd: &mut Validator,
    name: &str,
    winner: &mut Option<Token>,
    on_action: Option<&Token>,
    mut validate: impl FnMut(&Block, &Everything),
) {
    // Declarations arrive in reverse load order. Keep the first selected block for every report.
    let select = |key: &Token, block: &Block, data: &Everything| {
        if let Some(effective) = winner.as_ref() {
            if Game::is_ck3()
                && let Some(action) = on_action
            {
                warn(ErrorKey::OnActionConflict)
                    .strong()
                    .msg(format!("on-action `{action}` has competing `{name}` blocks"))
                    .info(
                        "To extend this hook, append a uniquely named child through `on_actions`.",
                    )
                    .loc_msg(key, "ignored block")
                    .loc_msg(effective, "winning block")
                    .push();
            }
        } else {
            *winner = Some(key.clone());
            validate(block, data);
        }
    };
    if Game::is_ck3() {
        vd.multi_field_validated_key_block(name, select);
    } else {
        vd.field_validated_key_block(name, select);
    }
}

fn validate_on_action_internal(
    block: &Block,
    data: &Everything,
    sc: &mut ScopeContext,
    winners: &mut OnActionBlocks,
    on_action: Option<&Token>,
) {
    let mut vd = Validator::new(block, data);
    if Game::is_jomini() {
        validate_single_block(
            &mut vd,
            "trigger",
            &mut winners.trigger,
            on_action,
            |block, data| {
                validate_trigger(block, data, sc, Tooltipped::No);
            },
        );
        vd.field_validated_block_sc("weight_multiplier", sc, validate_modifiers_with_base);
    }

    if Game::is_hoi4() {
        // Hoi4 allows defining an effect in each entry.
        vd.multi_field_validated_block("effect", |block, data| {
            validate_effect(block, data, sc, Tooltipped::No);
        });
    } else {
        validate_single_block(&mut vd, "effect", &mut winners.effect, on_action, |block, data| {
            validate_effect(block, data, sc, Tooltipped::No);
        });
    }

    // TODO: multiple random_events blocks in one on_action aren't outright bugged on Vic3,
    // but they might still get merged together into one big event pool. Verify.

    if Game::is_jomini() {
        // Several `events` blocks all fire in CK3 (in-game test, 1.20.1.2)
        vd.multi_field_validated_block("events", |b, data| {
            let mut vd = Validator::new(b, data);
            #[cfg(feature = "jomini")]
            if Game::is_jomini() {
                vd.multi_field_validated_block_sc("delay", sc, validate_duration);
            }
            for token in vd.values() {
                data.verify_exists(Item::Event, token);
                data.event_check_scope(token, sc);
                if let Some(mut event_sc) = sc.root_for_event(token, data) {
                    data.event_validate_call(token, &mut event_sc);
                }
            }
        });
    }
    let mut count = 0;
    #[allow(unused_variables)] // vic3 doesn't use `key`
    vd.multi_field_validated_key_block("random_events", |key, b, data| {
        let mut vd = Validator::new(b, data);
        #[cfg(feature = "jomini")]
        if Game::is_jomini() {
            vd.field_numeric("chance_to_happen"); // TODO: 0 - 100
            vd.field_script_value("chance_of_no_event", sc);
            vd.multi_field_validated_block_sc("delay", sc, validate_duration); // undocumented
        }
        for (_key, token) in vd.integer_values() {
            if token.is("0") {
                continue;
            }
            data.verify_exists(Item::Event, token);
            data.event_check_scope(token, sc);
            // Hoi4 uses the scope context directly.
            if Game::is_hoi4() {
                data.event_validate_call(token, sc);
            } else if let Some(mut event_sc) = sc.root_for_event(token, data) {
                data.event_validate_call(token, &mut event_sc);
            }
        }
        count += 1;
        #[cfg(feature = "ck3")] // Verified: this is only a problem in CK3
        if Game::is_ck3() && count == 2 {
            let msg = format!("multiple `{key}` blocks in one on_action do not work");
            let info = "try putting each into its own on_action and firing those separately";
            err(ErrorKey::Validation).msg(msg).info(info).loc(key).push();
        }
    });
    if Game::is_jomini() {
        // Several `first_valid` blocks in one on_action all work in CK3 (in-game test, 1.20.1.2)
        vd.multi_field_validated_block("first_valid", |b, data| {
            let mut vd = Validator::new(b, data);
            for token in vd.values() {
                data.verify_exists(Item::Event, token);
                data.event_check_scope(token, sc);
                if let Some(mut event_sc) = sc.root_for_event(token, data) {
                    data.event_validate_call(token, &mut event_sc);
                }
            }
        });
        // Several `on_actions` blocks all fire in CK3 (in-game test, 1.20.1.2)
        vd.multi_field_validated_block("on_actions", |b, data| {
            let mut vd = Validator::new(b, data);
            #[cfg(feature = "jomini")]
            if Game::is_jomini() {
                vd.multi_field_validated_block_sc("delay", sc, validate_duration);
            }
            for token in vd.values() {
                data.verify_exists(Item::OnAction, token);
                if let Some(mut action_sc) = sc.root_for_action(token, data) {
                    data.on_actions.validate_call(token, data, &mut action_sc);
                }
            }
        });
        // Several `random_on_action` blocks in one on_action all work in CK3 (in-game test, 1.20.1.2)
        vd.multi_field_validated_block("random_on_action", |b, data| {
            let mut vd = Validator::new(b, data);
            #[cfg(feature = "jomini")]
            if Game::is_jomini() {
                vd.field_numeric("chance_to_happen"); // TODO: 0 - 100
                vd.field_script_value("chance_of_no_event", sc);
            }
            for (_key, token) in vd.integer_values() {
                if token.is("0") {
                    continue;
                }
                data.verify_exists(Item::OnAction, token);
                if let Some(mut action_sc) = sc.root_for_action(token, data) {
                    data.on_actions.validate_call(token, data, &mut action_sc);
                }
            }
        });
        // Several `first_valid_on_action` blocks in one on_action all work in CK3 (in-game test, 1.20.1.2)
        vd.multi_field_validated_block("first_valid_on_action", |b, data| {
            let mut vd = Validator::new(b, data);
            for token in vd.values() {
                data.verify_exists(Item::OnAction, token);
                if let Some(mut action_sc) = sc.root_for_action(token, data) {
                    data.on_actions.validate_call(token, data, &mut action_sc);
                }
            }
        });
        // TODO: check for infinite fallback loops?
        vd.field_action("fallback", sc);
        #[cfg(feature = "ck3")]
        if Game::is_ck3()
            && let Some(token) = vd.field_value("fallback_event")
        {
            data.verify_exists(Item::Event, token);
            data.event_check_scope(token, sc);
            if let Some(mut event_sc) = sc.root_for_event(token, data) {
                data.event_validate_call(token, &mut event_sc);
            }
        }
    }
}

#[cfg(feature = "vic3")]
pub fn validate_on_action(block: &Block, data: &Everything, sc: &mut ScopeContext) {
    validate_on_action_internal(block, data, sc, &mut OnActionBlocks::default(), None);
}
