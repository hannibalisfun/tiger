use anyhow::Result;
use ck3_tiger::GAME_CONSTS;
use tiger_bin_shared::tiger;

fn main() -> Result<()> {
    let version = concat!(env!("CARGO_PKG_VERSION"), "+", env!("TIGER_REVISION"));
    tiger(GAME_CONSTS, version, env!("CARGO_PKG_NAME"))
}
