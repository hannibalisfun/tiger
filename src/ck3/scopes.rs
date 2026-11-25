use crate::everything::Everything;
use crate::scopes::Scopes;

/// Object scopes marked `Stores Variables: yes` in the CK3 1.21 engine export.
pub const VARIABLE_STORAGE: Scopes = Scopes::Character
    .union(Scopes::LandedTitle)
    .union(Scopes::Building)
    .union(Scopes::District)
    .union(Scopes::Activity)
    .union(Scopes::Secret)
    .union(Scopes::Province)
    .union(Scopes::Scheme)
    .union(Scopes::Combat)
    .union(Scopes::CombatSide)
    .union(Scopes::Faith)
    .union(Scopes::GreatHolyWar)
    .union(Scopes::Religion)
    .union(Scopes::War)
    .union(Scopes::StoryCycle)
    .union(Scopes::Dynasty)
    .union(Scopes::DynastyHouse)
    .union(Scopes::HouseRelation)
    .union(Scopes::Faction)
    .union(Scopes::Culture)
    .union(Scopes::Army)
    .union(Scopes::Artifact)
    .union(Scopes::Inspiration)
    .union(Scopes::Struggle)
    .union(Scopes::CharacterMemory)
    .union(Scopes::TravelPlan)
    .union(Scopes::Accolade)
    .union(Scopes::Rite)
    .union(Scopes::TaxSlot)
    .union(Scopes::Epidemic)
    .union(Scopes::Legend)
    .union(Scopes::Domicile)
    .union(Scopes::TaskContract)
    .union(Scopes::Situation)
    .union(Scopes::SituationParticipantGroup)
    .union(Scopes::SituationSubRegion)
    .union(Scopes::Confederation)
    .union(Scopes::GreatProject)
    .union(Scopes::TradeMarket)
    .union(Scopes::TradeRoute)
    .union(Scopes::TradeMonopoly);

// LAST UPDATED CK3 VERSION 1.16.0
pub fn needs_prefix(arg: &str, data: &Everything, scopes: Scopes) -> Option<&'static str> {
    use crate::item::Item;
    if scopes == Scopes::AccoladeType && data.item_exists(Item::AccoladeType, arg) {
        return Some("accolade_type");
    }
    if scopes == Scopes::ActivityType && data.item_exists(Item::ActivityType, arg) {
        return Some("activity_type");
    }
    if scopes == Scopes::BuildingType && data.item_exists(Item::Building, arg) {
        return Some("building_type");
    }
    if scopes == Scopes::Character && data.item_exists(Item::Character, arg) {
        return Some("character");
    }
    if scopes == Scopes::Culture && data.item_exists(Item::Culture, arg) {
        return Some("culture");
    }
    if scopes == Scopes::CulturePillar && data.item_exists(Item::CulturePillar, arg) {
        return Some("culture_pillar");
    }
    if scopes == Scopes::CultureTradition && data.item_exists(Item::CultureTradition, arg) {
        return Some("culture_tradition");
    }
    if scopes == Scopes::Decision && data.item_exists(Item::Decision, arg) {
        return Some("decision");
    }
    if scopes == Scopes::DistrictType && data.item_exists(Item::DistrictType, arg) {
        return Some("district_type");
    }
    if scopes == Scopes::Doctrine && data.item_exists(Item::Doctrine, arg) {
        return Some("doctrine");
    }
    if scopes == Scopes::Dynasty && data.item_exists(Item::Dynasty, arg) {
        return Some("dynasty");
    }
    if scopes == Scopes::EpidemicType && data.item_exists(Item::EpidemicType, arg) {
        return Some("epidemic_type");
    }
    if scopes == Scopes::Faith && data.item_exists(Item::Faith, arg) {
        return Some("faith");
    }
    if scopes == Scopes::Flag {
        return Some("flag");
    }
    if scopes == Scopes::GeographicalRegion && data.item_exists(Item::Region, arg) {
        return Some("geographical_region");
    }
    if scopes == Scopes::GovernmentType && data.item_exists(Item::GovernmentType, arg) {
        return Some("government_type");
    }
    if scopes == Scopes::HoldingType && data.item_exists(Item::HoldingType, arg) {
        return Some("holding_type");
    }
    if scopes == Scopes::HolySite && data.item_exists(Item::HolySite, arg) {
        return Some("holy_site");
    }
    if scopes == Scopes::HolySiteType && data.item_exists(Item::HolySite, arg) {
        return Some("holy_site_type");
    }
    if scopes == Scopes::DynastyHouse && data.item_exists(Item::House, arg) {
        return Some("house");
    }
    if scopes == Scopes::LegendType && data.item_exists(Item::LegendType, arg) {
        return Some("legend_type");
    }
    if scopes == Scopes::Province && data.item_exists(Item::Province, arg) {
        return Some("province");
    }
    if scopes == Scopes::Religion && data.item_exists(Item::Religion, arg) {
        return Some("religion");
    }
    if scopes == Scopes::Rite && data.item_exists(Item::RiteType, arg) {
        return Some("rite");
    }
    if scopes == Scopes::RiteType && data.item_exists(Item::RiteType, arg) {
        return Some("rite_type");
    }
    if scopes == Scopes::Struggle && data.item_exists(Item::Struggle, arg) {
        return Some("struggle");
    }
    if scopes == Scopes::TollLevel && data.item_exists(Item::TradeMarketTollLevel, arg) {
        return Some("toll_level");
    }
    if scopes == Scopes::TradeGoodCategory && data.item_exists(Item::TradeGoodCategory, arg) {
        return Some("trade_good_category");
    }
    if scopes == Scopes::TradeGoodType && data.item_exists(Item::TradeGoodType, arg) {
        return Some("trade_good_type");
    }
    if scopes == Scopes::Tenet && data.item_exists(Item::Tenet, arg) {
        return Some("tenet");
    }
    if scopes == Scopes::LandedTitle && data.item_exists(Item::Title, arg) {
        return Some("title");
    }
    if scopes == Scopes::VassalContract && data.item_exists(Item::SubjectContract, arg) {
        return Some("vassal_contract");
    }
    None
}
