use super::buildings::validate_asset;
use crate::block::Block;
use crate::ck3::validate::validate_cost;
use crate::context::ScopeContext;
use crate::db::{Db, DbKind};
use crate::everything::Everything;
use crate::game::GameFlags;
use crate::item::{Item, ItemLoader};
use crate::scopes::Scopes;
use crate::token::Token;
use crate::tooltipped::Tooltipped;
use crate::validate::validate_possibly_named_color;
use crate::validator::Validator;

pub(crate) const DISTRICT_SLOT_TYPES: &[&str] = &[
    "regular",
    "holding",
    "duchy_capital",
    "special",
    "great_district",
    "resource",
    "trade_post",
    "entrepot",
];

#[derive(Clone, Debug)]
struct DistrictType;

inventory::submit! {
    ItemLoader::Normal(GameFlags::Ck3, Item::DistrictType, DistrictType::add)
}

impl DistrictType {
    fn add(db: &mut Db, key: Token, block: Block) {
        db.add(Item::DistrictType, key, block, Box::new(Self));
    }
}

fn district_province(key: &Token) -> ScopeContext {
    let mut sc = ScopeContext::new(Scopes::Province, key);
    sc.define_name("holder", Scopes::Character, key);
    sc.define_name("county", Scopes::LandedTitle, key);
    sc
}

fn validate_district_name(key: &Token, data: &Everything) {
    data.verify_exists(Item::Localization, key);
    data.mark_used(Item::Localization, &format!("{key}_desc"));
    data.mark_used(Item::Localization, &format!("{key}_desc_flavor"));
}

impl DbKind for DistrictType {
    fn validate(&self, key: &Token, block: &Block, data: &Everything) {
        validate_district_name(key, data);
        let mut vd = Validator::new(block, data);
        vd.field_choice("type", DISTRICT_SLOT_TYPES);
        vd.req_field("primary_building");
        vd.field_item("primary_building", Item::Building);
        vd.field_list_items("buildings", Item::Building);
        vd.field_item("unlocked_by", Item::DistrictType);
        vd.field_validated("color", validate_possibly_named_color);
        vd.field_bool("can_be_replaced");
        vd.field_bool("show_disabled");

        for (field, item) in [("triggered_name", Item::Localization), ("background", Item::File)] {
            vd.multi_field_validated_block(field, |block, data| {
                let mut vd = Validator::new(block, data);
                vd.field_trigger_rooted("trigger", Tooltipped::No, Scopes::Province);
                vd.req_field("reference");
                if item == Item::Localization {
                    if let Some(reference) = vd.field_value("reference") {
                        validate_district_name(reference, data);
                    }
                } else {
                    vd.field_item("reference", item);
                }
            });
        }
        vd.multi_field_validated_block("asset", validate_asset);
        vd.multi_field_validated_block("assets", |block, data| {
            Validator::new(block, data).multi_field_validated_block("asset", validate_asset);
        });

        vd.field_trigger_builder("is_enabled", Tooltipped::FailuresOnly, district_province);
        let potential_tip = if block.get_field_bool("show_disabled").unwrap_or(false) {
            Tooltipped::FailuresOnly
        } else {
            Tooltipped::No
        };
        vd.field_trigger_builder("can_construct_potential", potential_tip, district_province);
        vd.field_trigger_builder(
            "can_construct_showing_failures_only",
            Tooltipped::FailuresOnly,
            district_province,
        );
        vd.field_trigger_builder("can_construct", Tooltipped::Yes, district_province);

        if block.field_value_is("type", "great_district") {
            vd.field_trigger_builder("can_rebuild", Tooltipped::Yes, district_province);
            vd.field_validated_block_rooted("rebuild_cost", Scopes::Character, validate_cost);
        } else {
            vd.ban_field("can_rebuild", || "great districts");
            vd.ban_field("rebuild_cost", || "great districts");
        }
    }
}
