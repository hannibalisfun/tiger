use std::path::PathBuf;

use crate::block::{BV, Block};
use crate::ck3::data::provinces::ProvId;
use crate::ck3::data::titles::Titles;
use crate::date::Date;
use crate::everything::Everything;
use crate::fileset::{FileEntry, FileHandler};
use crate::helpers::{TigerHashMap, dup_error};
use crate::item::Item;
use crate::parse::ParserMemory;
use crate::pdxfile::PdxFile;
use crate::report::{ErrorKey, Severity, err, warn};
use crate::scopes::Scopes;
use crate::token::Token;
use crate::tooltipped::Tooltipped;
use crate::validator::Validator;
use crate::variables::Variables;

#[derive(Clone, Debug, Default)]
pub struct ProvinceHistories {
    provinces: TigerHashMap<ProvId, ProvinceHistory>,
}

impl ProvinceHistories {
    fn load_item(&mut self, id: ProvId, key: Token, mut block: Block) {
        if let Some(province) = self.provinces.get_mut(&id) {
            // Multiple entries are valid but could easily be a mistake.
            if province.key.loc.kind >= key.loc.kind {
                dup_error(&key, &province.key, "province");
            }
            province.block.append(&mut block);
        } else {
            self.provinces.insert(id, ProvinceHistory::new(key, block));
        }
    }

    pub fn scan_variables(&self, registry: &mut Variables) {
        for item in self.provinces.values() {
            registry.scan(&item.block);
        }
    }

    pub fn validate(&self, data: &Everything) {
        for (provid, item) in &self.provinces {
            item.validate(*provid, data);
        }
    }

    pub fn check_pod_faiths(&self, data: &Everything, titles: &Titles) {
        for bookmark in [
            Date::new(1230, 1, 4),
            Date::new(1230, 1, 5),
            Date::new(1230, 1, 6),
            Date::new(1375, 7, 5),
            Date::new(1510, 1, 3),
        ] {
            for (provid, provhist) in &self.provinces {
                if let Some(capital) = titles.capital_of(*provid) {
                    let religion = provhist.block.get_field_at_date("religion", bookmark);
                    if let Some(religion) = religion.and_then(BV::get_value) {
                        if !data.item_has_property(Item::Faith, religion.as_str(), "is_modded") {
                            let msg = format!(
                                "Vanilla or unknown religion in prov {} (county {}) at {}",
                                provhist.key, capital, bookmark
                            );
                            warn(ErrorKey::PrincesOfDarkness).msg(msg).loc(religion).push();
                        }
                    } else {
                        warn(ErrorKey::PrincesOfDarkness)
                            .msg("no religion")
                            .loc(&provhist.key)
                            .push();
                    }
                }
            }
        }
    }
}

impl FileHandler<Block> for ProvinceHistories {
    fn subpath(&self) -> PathBuf {
        PathBuf::from("history/provinces")
    }

    fn load_file(&self, entry: &FileEntry, parser: &ParserMemory) -> Option<Block> {
        if !entry.filename().to_string_lossy().ends_with(".txt") {
            return None;
        }

        PdxFile::read_detect_encoding(entry, parser)
    }

    fn handle_file(&mut self, _entry: &FileEntry, mut block: Block) {
        for (key, block) in block.drain_definitions_warn() {
            if let Ok(id) = key.as_str().parse() {
                self.load_item(id, key, block);
            } else {
                let msg = "unexpected key, expected only province ids";
                warn(ErrorKey::Validation).msg(msg).loc(key).push();
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProvinceHistory {
    key: Token,
    block: Block,
}

impl ProvinceHistory {
    fn new(key: Token, block: Block) -> Self {
        Self { key, block }
    }

    fn district_slot(data: &Everything, district: &Token, slot: &str) {
        data.verify_exists(Item::DistrictType, district);
        if let Some((_, definition)) = data.get_key_block(Item::DistrictType, district.as_str())
            && !definition.field_value_is("type", slot)
        {
            err(ErrorKey::Validation)
                .msg(format!("district `{district}` requires type `{slot}` for this slot"))
                .loc(district)
                .push();
        }
    }

    fn validate_district(block: &Block, data: &Everything, holding: Option<&Token>) {
        let mut vd = Validator::new(block, data);
        vd.req_field("type");
        if let Some(district) = vd.field_value("type") {
            data.verify_exists(Item::DistrictType, district);
            if let Some((_, definition)) = data.get_key_block(Item::DistrictType, district.as_str())
            {
                if definition.field_value_is("type", "trade_post") {
                    err(ErrorKey::Validation)
                        .msg("use the `trade_post` history field for a trade post district")
                        .loc(district)
                        .push();
                } else if definition.field_value_is("type", "holding")
                    && let Some(holding) = holding
                    && let Some((_, definition)) =
                        data.get_key_block(Item::HoldingType, holding.as_str())
                    && let Some(primary) = definition.get_field_value("primary_district")
                    && !primary.is(district.as_str())
                {
                    err(ErrorKey::Validation)
                        .msg(format!(
                            "holding `{holding}` uses `{primary}` as its holding district"
                        ))
                        .loc(district)
                        .push();
                }
            }
        }
        vd.field_integer("level");
        vd.field_list_items("buildings", Item::Building);
    }

    fn validate_trade_post(block: &Block, data: &Everything) {
        let mut vd = Validator::new(block, data);
        vd.req_field("type");
        vd.req_field("owner");
        if let Some(district) = vd.field_value("type") {
            Self::district_slot(data, district, "trade_post");
        }
        vd.field_item("owner", Item::Character);
        vd.field_integer("level");
        vd.field_list_items("buildings", Item::Building);
    }

    fn validate_common(vd: &mut Validator, data: &Everything, holding: Option<&Token>) {
        vd.field_item("culture", Item::Culture);
        vd.field_item("faith", Item::Faith);
        // takes priority over faith
        vd.field_item("rite", Item::RiteType);
        if let Some(token) = vd.field_value("holding")
            && !token.is("auto")
            && !token.is("none")
        {
            data.verify_exists(Item::HoldingType, token);
        }
        vd.field_list_items("buildings", Item::Building);
        // The history schema still accepts building keys with the legacy slot fields.
        vd.multi_field_item("special_building_slot", Item::Building);
        vd.multi_field_item("special_building", Item::Building);
        // TODO: check if province is duchy capital
        // TODO: check if building is duchy capital building
        vd.field_item("duchy_capital_building", Item::Building);

        vd.field_integer("province_size");
        for (field, slot) in
            [("special_district_slot", "special"), ("entrepot_district_slot", "entrepot")]
        {
            for district in vd.multi_field_value(field) {
                Self::district_slot(data, district, slot);
            }
        }
        vd.multi_field_validated_block("districts", |block, data| {
            let mut vd = Validator::new(block, data);
            if vd.multi_field_validated_block("district", |block, data| {
                Self::validate_district(block, data, holding);
            }) && holding.is_none_or(|h| h.is("none") || h.is("auto"))
            {
                err(ErrorKey::Validation)
                    .msg("districts require an explicit holding type at this date")
                    .loc(block)
                    .push();
            }
        });
        vd.multi_field_validated_block("trade_post", Self::validate_trade_post);
        vd.field_effect_rooted("effect", Tooltipped::No, Scopes::Province);
    }

    fn validate(&self, provid: ProvId, data: &Everything) {
        data.provinces_ck3.verify_exists_provid(provid, &self.key, Severity::Error);
        // TODO: verify that all county-capital provinces have a culture and religion
        // This needs province mappings to be loaded too
        let mut vd = Validator::new(&self.block, data);
        let mut holding = self.block.get_field_value("holding");
        Self::validate_common(&mut vd, data, holding);
        vd.advice_field("terrain", "removed in 1.20");
        // Dates may be out of order across files. Preserve merged file order for equal dates.
        let mut entries = Vec::new();
        vd.validate_history_blocks(|date, _, block, _| entries.push((date, block)));
        entries.sort_by_key(|(date, _)| *date);
        for (_, block) in entries {
            holding = block.get_field_value("holding").or(holding);
            let mut dated = Validator::new(block, data);
            Self::validate_common(&mut dated, data, holding);
        }
    }
}
