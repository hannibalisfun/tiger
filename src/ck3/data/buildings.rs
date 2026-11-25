use crate::block::{BV, Block};
use crate::ck3::modif::ModifKinds;
use crate::ck3::validate::validate_cost;
use crate::context::ScopeContext;
use crate::db::{Db, DbKind};
use crate::desc::validate_desc;
use crate::everything::Everything;
use crate::game::GameFlags;
use crate::item::{Item, ItemLoader};
use crate::modif::validate_modifs;
use crate::scopes::Scopes;
use crate::script_value::validate_non_dynamic_script_value;
use crate::token::Token;
use crate::tooltipped::Tooltipped;
use crate::validate::validate_modifiers_with_base;
use crate::validator::Validator;

#[derive(Clone, Debug)]
pub struct Building {
    is_upgrade: bool,
}

inventory::submit! {
    ItemLoader::Normal(GameFlags::Ck3, Item::Building, Building::add)
}

impl Building {
    pub fn new() -> Self {
        Self { is_upgrade: false }
    }

    pub fn add(db: &mut Db, key: Token, block: Block) {
        db.add(Item::Building, key, block, Box::new(Self::new()));
    }

    pub fn finalize(db: &mut Db) {
        let mut upgrades = Vec::new();
        for (_, block) in db.iter_key_block(Item::Building) {
            if let Some(token) = block.get_field_value("next_building") {
                upgrades.push(token.to_string());
            }
        }
        for upgrade in upgrades {
            db.set_property(Item::Building, &upgrade, "is_upgrade");
        }
    }
}

impl DbKind for Building {
    fn add_subitems(&self, key: &Token, block: &Block, db: &mut Db) {
        for token in block.get_field_values("flag") {
            db.add_flag(Item::BuildingFlag, token.clone());
        }
        if block.field_value_is("type", "special") {
            db.add_flag(Item::SpecialBuilding, key.clone());
        }
    }

    fn validate(&self, key: &Token, block: &Block, data: &Everything) {
        fn sc_builder(key: &Token) -> ScopeContext {
            let mut sc = ScopeContext::new(Scopes::Province, key);
            sc.define_name("holder", Scopes::Character, key);
            sc.define_name("county", Scopes::LandedTitle, key);
            sc
        }

        let mut vd = Validator::new(block, data);

        let graphical_only = block.get_field_bool("is_graphical_background").unwrap_or(false);
        if !graphical_only {
            data.verify_exists(Item::Localization, key);
            // A tier can use the first building's description, or have no description.
            data.mark_used(Item::Localization, &format!("{key}_desc"));
            if !self.is_upgrade {
                data.mark_used(Item::Localization, &format!("building_type_{key}"));
                data.mark_used(Item::Localization, &format!("building_type_{key}_desc"));
            }
        }

        vd.field_icon("type_icon", "NGameIcons|BUILDING_TYPE_ICON_PATH", "");

        for field in ["levy", "max_garrison", "construction_time", "garrison_reinforcement_factor"]
        {
            vd.field_validated_value(field, |_, mut value| {
                if !value.value().is_number() {
                    value.item(Item::ScriptValue);
                    data.script_values.validate_non_dynamic_call(value.value(), data);
                } else if field == "garrison_reinforcement_factor" {
                    value.numeric_range(0.0..=1.0);
                } else {
                    value.integer();
                }
            });
        }
        vd.replaced_field("type", "the parent district's type in common/districts");
        vd.field_script_value_builder("district_level_requirement", sc_builder);
        vd.multi_field_validated_key_block("triggered_name", |key, block, data| {
            let mut vd = Validator::new(block, data);
            vd.field_trigger_rooted("trigger", Tooltipped::No, Scopes::Province);
            vd.req_field("reference");
            vd.field_localization("reference", &mut ScopeContext::new(Scopes::Province, key));
        });

        vd.multi_field_validated_block("assets", |block, data| {
            let mut vd = Validator::new(block, data);
            vd.multi_field_validated_block("asset", validate_asset);
        });
        vd.multi_field_validated_block("asset", validate_asset);

        vd.field_trigger_builder(
            "is_enabled",
            if graphical_only { Tooltipped::No } else { Tooltipped::FailuresOnly },
            sc_builder,
        );

        vd.field_trigger_builder("can_rebuild", Tooltipped::Yes, sc_builder);

        // For buildings that are upgrades, can_construct_potential is added to can_construct_showing_failures_only so it will be tooltipped
        vd.field_trigger_builder(
            "can_construct_potential",
            if block.get_field_bool("show_disabled").unwrap_or(false) || self.is_upgrade {
                Tooltipped::FailuresOnly
            } else {
                Tooltipped::No
            },
            sc_builder,
        );

        vd.field_trigger_builder(
            "can_construct_showing_failures_only",
            Tooltipped::FailuresOnly,
            sc_builder,
        );

        vd.field_trigger_builder("can_construct", Tooltipped::Yes, sc_builder);
        vd.field_bool("show_disabled");

        vd.field_script_value_rooted("cost_gold", Scopes::Character);
        vd.field_script_value_rooted("cost_piety", Scopes::Character);
        vd.field_script_value_rooted("cost_prestige", Scopes::Character);
        vd.field_validated_block("cost", validate_fixed_cost);

        vd.field_item("next_building", Item::Building);
        vd.field_validated_build_sc("effect_desc", sc_builder, validate_desc);

        validate_modifiers(&mut vd);
        if block.has_key("fallback") {
            vd.req_field("is_enabled");
        }
        vd.field_validated_block("fallback", |block, data| {
            validate_modifiers(&mut Validator::new(block, data));
        });
        vd.multi_field_validated_block("trade_good_need", validate_trade_good_need);

        vd.multi_field_value("flag");

        vd.field_validated_key("ai_value", |key, bv, data| match bv {
            BV::Value(token) => {
                token.expect_integer();
            }
            BV::Block(block) => {
                let mut sc = sc_builder(key);
                validate_modifiers_with_base(block, data, &mut sc);
            }
        });

        vd.field_bool("is_graphical_background");

        for field in ["on_start", "on_cancelled", "on_complete"] {
            vd.field_effect_builder(field, Tooltipped::No, |key| {
                let mut sc = ScopeContext::new(Scopes::Province, key);
                sc.define_name("character", Scopes::Character, key);
                sc.define_name("holding", Scopes::HoldingType, key);
                sc
            });
        }

        vd.field_item("great_project_type", Item::GreatProjectType);
        vd.field_validated_block_rooted("rebuild_cost", Scopes::Character, validate_cost);
        vd.field_bool("is_mandala_capital");

        vd.multi_field_validated_block("override_modifiers", |block, data| {
            let mut vd = Validator::new(block, data);
            vd.req_field("requires_dlc_flag");
            vd.field_item("requires_dlc_flag", Item::DlcFeature);
            validate_modifiers(&mut vd);
        });
    }

    fn set_property(&mut self, _key: &Token, _block: &Block, property: &str) {
        if property == "is_upgrade" {
            self.is_upgrade = true;
        }
    }
}

/// A cost made of numbers or named script values, not of dynamic script values
fn validate_fixed_cost(block: &Block, data: &Everything) {
    let mut vd = Validator::new(block, data);
    for field in [
        "gold",
        "prestige",
        "piety",
        "renown",
        "influence",
        "herd",
        "treasury",
        "treasury_or_gold",
        "merit",
        "barter_goods",
    ] {
        vd.field_validated_value(field, |_, mut vd| {
            if vd.value().get_number().is_some() {
                vd.numeric();
            } else {
                vd.item(Item::ScriptValue);
            }
        });
    }
}

pub(super) fn validate_asset(block: &Block, data: &Everything) {
    let mut vd = Validator::new(block, data);
    vd.req_field("type");
    vd.field_choice("type", &["pdxmesh", "entity"]);

    let meshes = block.field_value_is("type", "pdxmesh");
    let itype = if meshes { Item::Pdxmesh } else { Item::Entity };

    vd.multi_field_item("name", itype);
    vd.field_list_items("names", itype);

    vd.field_item("domicile_building", Item::DomicileBuilding);

    vd.field_item("illustration", Item::File);

    vd.multi_field_validated("soundeffect", |bv, data| match bv {
        BV::Value(token) => data.verify_exists(Item::Sound, token),
        BV::Block(block) => {
            let mut vd = Validator::new(block, data);
            vd.req_field("soundeffect");
            vd.field_item("soundeffect", Item::Sound);
            vd.field_validated_block("soundparameter", |block, data| {
                Validator::new(block, data).unknown_value_fields(|_, value| {
                    value.expect_number();
                });
            });
        }
    });

    vd.field_list_items("graphical_cultures", Item::BuildingGfx);
    vd.field_list_items("graphical_faiths", Item::GraphicalFaith);
    vd.field_list_items("graphical_regions", Item::Region); // TODO check that it's a graphical region
    vd.field_list_items("provinces", Item::Province);
    vd.field_list_items("governments", Item::GovernmentType);

    vd.field_item("requires_dlc_flag", Item::DlcFeature);
}

fn validate_trade_good_need(block: &Block, data: &Everything) {
    let mut vd = Validator::new(block, data);
    vd.req_field_one_of(&["trade_good", "trade_good_category"]);
    vd.field_item("trade_good", Item::TradeGoodType);
    vd.field_item("trade_good_category", Item::TradeGoodCategory);
    vd.field_validated("amount", validate_non_dynamic_script_value);
    // The trade good schema and vanilla entrepot buildings use this spelling.
    vd.field_bool("skip_internal_fulfillment");
    vd.advice_field("skip_internal_fullfilment", "vanilla uses skip_internal_fulfillment");
    vd.field_bool("skip_internal_fullfilment");

    for (field, item) in [
        ("culture_parameter", Item::CultureParameter),
        ("not_culture_parameter", Item::CultureParameter),
        ("rite_parameter", Item::DoctrineParameter),
        ("not_rite_parameter", Item::DoctrineParameter),
        ("government_flag", Item::GovernmentFlag),
        ("situation_parameter", Item::SituationPhaseParameter),
        ("dynasty_perk", Item::DynastyPerk),
        ("terrain", Item::Terrain),
        ("potential_trade_good", Item::TradeGoodType),
    ] {
        vd.field_item(field, item);
    }
    vd.field_integer("province_size");
    vd.field_bool("is_coastal");
    vd.field_bool("is_riverside");
    vd.field_validated_block("modifiers", |block, data| {
        validate_modifiers(&mut Validator::new(block, data));
    });
}

fn validate_modifiers(vd: &mut Validator) {
    use ModifKinds as K;

    for (field, kinds) in [
        ("character_modifier", K::Character),
        ("province_modifier", K::Province),
        ("county_modifier", K::County),
        ("duchy_capital_county_modifier", K::County),
        ("county_holder_character_modifier", K::Character),
        ("market_modifier", K::TradeMarket),
    ] {
        vd.multi_field_validated_block(field, |block, data| {
            validate_modifs(block, data, kinds, Validator::new(block, data));
        });
    }

    for (field, kinds, parameter) in [
        ("character_culture_modifier", K::Character, Item::CultureParameter),
        ("character_rite_modifier", K::Character, Item::DoctrineParameter),
        ("character_situation_modifier", K::Character, Item::SituationPhaseParameter),
        ("character_government_modifier", K::Character, Item::GovernmentFlag),
        ("province_culture_modifier", K::Province, Item::CultureParameter),
        ("province_rite_modifier", K::Province, Item::DoctrineParameter),
        ("province_situation_modifier", K::Province, Item::SituationPhaseParameter),
        ("province_government_modifier", K::Province, Item::GovernmentFlag),
        ("county_culture_modifier", K::County, Item::CultureParameter),
        ("county_rite_modifier", K::County, Item::DoctrineParameter),
        ("county_situation_modifier", K::County, Item::SituationPhaseParameter),
        ("duchy_capital_county_culture_modifier", K::County, Item::CultureParameter),
        ("duchy_capital_county_rite_modifier", K::County, Item::DoctrineParameter),
        ("duchy_capital_county_situation_modifier", K::County, Item::SituationPhaseParameter),
        ("market_culture_modifier", K::TradeMarket, Item::CultureParameter),
        ("market_faith_modifier", K::TradeMarket, Item::DoctrineParameter),
        ("market_rite_modifier", K::TradeMarket, Item::DoctrineParameter),
        ("market_situation_modifier", K::TradeMarket, Item::SituationPhaseParameter),
        ("market_government_modifier", K::TradeMarket, Item::GovernmentFlag),
    ] {
        vd.multi_field_validated_block(field, |block, data| {
            let mut vd = Validator::new(block, data);
            vd.req_field_one_of(&["parameter", "not_parameter"]);
            vd.field_item("parameter", parameter);
            vd.field_item("not_parameter", parameter);
            validate_modifs(block, data, kinds, vd);
        });
    }

    for (field, kinds) in [
        ("character_dynasty_modifier", K::Character),
        ("province_dynasty_modifier", K::Province),
        ("county_dynasty_modifier", K::County),
        ("duchy_capital_county_dynasty_modifier", K::County),
        ("market_dynasty_modifier", K::TradeMarket),
    ] {
        vd.multi_field_validated_block(field, |block, data| {
            let mut vd = Validator::new(block, data);
            vd.req_field("county_holder_dynasty_perk");
            vd.field_item("county_holder_dynasty_perk", Item::DynastyPerk);
            validate_modifs(block, data, kinds, vd);
        });
    }

    for (field, kinds) in [
        ("character_potential_trade_good_modifier", K::Character),
        ("province_potential_trade_good_modifier", K::Province),
        ("county_potential_trade_good_modifier", K::County),
    ] {
        vd.multi_field_validated_block(field, |block, data| {
            let mut vd = Validator::new(block, data);
            vd.req_field("trade_good");
            vd.field_item("trade_good", Item::TradeGoodType);
            validate_modifs(block, data, kinds, vd);
        });
    }

    for (field, kinds) in
        [("province_terrain_modifier", K::Province), ("market_terrain_modifier", K::TradeMarket)]
    {
        vd.multi_field_validated_block(field, |block, data| {
            let mut vd = Validator::new(block, data);
            vd.exclusive_fields(&["parameter", "not_parameter"]);
            vd.field_item("parameter", Item::CultureParameter);
            vd.field_item("not_parameter", Item::CultureParameter);
            vd.field_item("terrain", Item::Terrain);
            vd.field_bool("is_coastal");
            vd.field_bool("is_riverside");
            validate_modifs(block, data, kinds, vd);
        });
    }
    vd.multi_field_validated_block("market_size_modifier", |block, data| {
        let mut vd = Validator::new(block, data);
        vd.req_field("province_size");
        vd.field_integer("province_size");
        validate_modifs(block, data, K::TradeMarket, vd);
    });
    vd.multi_field_validated_block("county_holding_modifier", |block, data| {
        let mut vd = Validator::new(block, data);
        vd.req_field("holding");
        vd.field_item("holding", Item::HoldingType);
        // This applies to each matching holding's province within the county.
        validate_modifs(block, data, K::Province, vd);
    });

    for (old, new) in [
        ("character_faith_modifier", "character_rite_modifier"),
        ("province_faith_modifier", "province_rite_modifier"),
        ("county_faith_modifier", "county_rite_modifier"),
        ("duchy_capital_county_faith_modifier", "duchy_capital_county_rite_modifier"),
    ] {
        vd.replaced_field(old, new);
    }
}
