use crate::block::Block;
use crate::db::Db;
use crate::game::GameFlags;
use crate::item::{Item, ItemLoader};
use crate::token::Token;

// Building needs and production modifiers refer to both of these databases.
inventory::submit! {
    ItemLoader::Normal(GameFlags::Ck3, Item::TradeGoodType, add_good)
}
inventory::submit! {
    ItemLoader::Normal(GameFlags::Ck3, Item::TradeGoodCategory, add_category)
}

fn add_good(db: &mut Db, key: Token, _block: Block) {
    db.add_flag(Item::TradeGoodType, key);
}

fn add_category(db: &mut Db, key: Token, _block: Block) {
    db.add_flag(Item::TradeGoodCategory, key);
}
