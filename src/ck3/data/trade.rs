use crate::block::Block;
use crate::ck3::modif::ModifKinds;
use crate::context::ScopeContext;
use crate::db::DbKind;
use crate::everything::Everything;
use crate::game::GameFlags;
use crate::item::{Item, ItemLoader};
use crate::modif::validate_modifs;
use crate::report::{ErrorKey, err};
use crate::scopes::Scopes;
use crate::script_value::validate_non_dynamic_script_value;
use crate::token::Token;
use crate::validate::validate_color;
use crate::validator::Validator;

#[derive(Debug)]
struct TradeData(fn(&Token, &Block, &Everything));

impl DbKind for TradeData {
    fn validate(&self, key: &Token, block: &Block, data: &Everything) {
        (self.0)(key, block, data);
    }
}

macro_rules! trade_database {
    ($item:ident, $validate:ident) => {
        inventory::submit! {
            ItemLoader::Normal(GameFlags::Ck3, Item::$item, |db, key, block| {
                db.add(Item::$item, key, block, Box::new(TradeData($validate)));
            })
        }
    };
}

trade_database!(TradeGoodType, good);
trade_database!(TradeGoodCategory, category);
trade_database!(CountyPotentialTradeGoods, county_goods);
trade_database!(TradeMarketTollLevel, toll);
trade_database!(TradeTask, task);
trade_database!(TradeGoodGraphics, graphics);
trade_database!(Road, road);
trade_database!(RoadModifier, road_modifiers);

fn name_and_desc(key: &Token, data: &Everything) {
    data.verify_exists(Item::Localization, key);
    data.verify_exists_implied(Item::Localization, &format!("{key}_desc"), key);
}

fn good(key: &Token, block: &Block, data: &Everything) {
    name_and_desc(key, data);
    let mut vd = Validator::new(block, data);
    // Vanilla prices include fractional gold, despite the integer example in the schema.
    vd.field_numeric("base_price");
    vd.field_list_integers_exactly("origin", 2);
    vd.field_value("category"); // Integer flag, independent of the trade category databases.
    vd.field_bool("is_basic");
}

fn modifier_field(vd: &mut Validator, name: &str, kinds: ModifKinds) {
    vd.field_validated_block(name, |block, data| {
        validate_modifs(block, data, kinds, Validator::new(block, data));
    });
}

fn category_members(block: &Block) -> impl Iterator<Item = &Token> {
    block
        .iter_values()
        .chain(block.get_field_block("trade_goods").into_iter().flat_map(Block::iter_values))
}

fn category(key: &Token, block: &Block, data: &Everything) {
    name_and_desc(key, data);
    let mut vd = Validator::new(block, data);
    vd.field_bool("need");
    vd.field_validated_block("color", validate_color);
    // The .info shows bare keys; shipped categories use a trade_goods list.
    for member in vd.values() {
        data.verify_exists(Item::TradeGoodType, member);
    }
    vd.field_list_items("trade_goods", Item::TradeGoodType);
    if block.get_field_bool("need") == Some(true) {
        // Validate effective definitions after filename replacement and item overrides.
        let order = |t: &Token| (t.loc.kind, t.loc.pathname(), t.loc.line, t.loc.column);
        let mut earlier: Vec<_> = data
            .database
            .iter_key_block(Item::TradeGoodCategory)
            .filter(|(other, b)| {
                order(other) < order(key) && b.get_field_bool("need") == Some(true)
            })
            .collect();
        earlier.sort_unstable_by_key(|(other, _)| order(other));
        for member in category_members(block) {
            if let Some((other, _)) =
                earlier.iter().find(|(_, b)| category_members(b).any(|t| t.is(member.as_str())))
            {
                err(ErrorKey::Validation)
                    .msg(format!("{member} already belongs to need category {other}"))
                    .loc(member)
                    .loc_msg(*other, "other need category")
                    .push();
            }
        }
    }
    if block.has_key("per_level_interval") || block.has_key("milestone_interval") {
        vd.req_field("levels");
    }
    modifier_field(&mut vd, "per_level_interval", ModifKinds::Character);
    vd.multi_field_validated_block("milestone_interval", |block, data| {
        let mut vd = Validator::new(block, data);
        vd.field_integer_range("every_level", 1..=i64::from(i32::MAX));
        validate_modifs(block, data, ModifKinds::Character, vd);
    });
    vd.field_validated_block("levels", |block, data| {
        Validator::new(block, data).unknown_block_fields(|_, level| {
            modifier_field(
                &mut Validator::new(level, data),
                "character_modifier",
                ModifKinds::Character,
            );
        });
    });
}

fn county_goods(key: &Token, block: &Block, data: &Everything) {
    data.verify_exists(Item::TradeGoodType, key);
    let mut vd = Validator::new(block, data);
    vd.req_field("counties");
    vd.field_validated_list("counties", |county, data| {
        data.verify_exists(Item::Title, county);
        if !county.starts_with("c_") {
            err(ErrorKey::TitleTier).msg("expected a county title").loc(county).push();
        }
    });
}

fn toll(key: &Token, block: &Block, data: &Everything) {
    data.verify_exists(Item::Localization, key);
    let mut vd = Validator::new(block, data);
    for field in ["route_owner_toll_rate", "market_owner_toll_rate"] {
        vd.field_numeric_range(field, 0.0..=1.0);
    }
    vd.field_script_value_builder("ai_market_owner_desire", |token| {
        let mut sc = ScopeContext::new(Scopes::Character, token);
        sc.define_name("trade_market", Scopes::TradeMarket, token);
        sc
    });
}

fn task(key: &Token, block: &Block, data: &Everything) {
    name_and_desc(key, data);
    let mut vd = Validator::new(block, data);
    vd.field_bool("default");
    vd.field_script_value_rooted("aptitude", Scopes::Character);
    vd.field_list_integers_exactly("aptitude_level_breakpoints", 4);
    vd.field_item("icon", Item::File);
    vd.field_item("animation", Item::PortraitAnimation);
    vd.field_validated_block("scaling_modifiers", |block, data| {
        let mut vd = Validator::new(block, data);
        for tier in ["terrible", "poor", "average", "good", "excellent"] {
            modifier_field(&mut vd, tier, ModifKinds::TradeRoute);
        }
    });
}

fn graphics(_: &Token, block: &Block, data: &Everything) {
    let mut vd = Validator::new(block, data);
    vd.req_field("trade_good_type");
    vd.field_item("trade_good_type", Item::TradeGoodType);
    vd.field_validated("minimum_produced", validate_non_dynamic_script_value);
    vd.field_list_items("graphical_culture", Item::BuildingGfx);
    vd.field_item("asset", Item::Pdxmesh);
}

fn road(_: &Token, block: &Block, data: &Everything) {
    let mut vd = Validator::new(block, data);
    vd.field_integer_range("tier", 0..=5);
    vd.field_integer_range("spline_asset_id", 0..=1);
    vd.req_field("provinces");
    vd.field_validated_block("provinces", |block, data| {
        let mut vd = Validator::new(block, data);
        let provinces = vd.values();
        if provinces.len() < 2 {
            err(ErrorKey::Validation).msg("a road needs at least two provinces").loc(block).push();
        }
        for province in provinces {
            data.verify_exists(Item::Province, province);
        }
    });
}

fn road_effects(vd: &mut Validator) {
    modifier_field(vd, "modifier", ModifKinds::Road);
    modifier_field(vd, "province_modifier", ModifKinds::Province);
    modifier_field(vd, "trade_route_modifier", ModifKinds::TradeRoute);
}

fn road_modifiers(_: &Token, block: &Block, data: &Everything) {
    let mut vd = Validator::new(block, data);
    vd.field_list_items("type", Item::Road);
    road_effects(&mut vd);
    for tier in ["tier_0", "tier_1", "tier_2", "tier_3", "tier_4", "tier_5"] {
        vd.field_validated_block(tier, |block, data| {
            let mut vd = Validator::new(block, data);
            road_effects(&mut vd);
            modifier_field(&mut vd, "travel_modifier", ModifKinds::TravelPlan);
            modifier_field(&mut vd, "character_modifier", ModifKinds::Character);
        });
    }
}
