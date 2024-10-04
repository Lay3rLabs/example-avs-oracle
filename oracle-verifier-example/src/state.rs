use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Decimal, Uint128};
use cw_storage_plus::{Item, Map};
use lavs_apis::{id::TaskId, verifier_simple::TaskMetadata};

pub const CONFIG: Item<Config> = Item::new("config");
pub const VOTES: Map<(&Addr, TaskId, &Addr), OperatorVote> = Map::new("operator_votes");
pub const TASKS: Map<(&Addr, TaskId), TaskMetadata> = Map::new("tasks");
pub const SLASHED_OPERATORS: Map<&Addr, bool> = Map::new("slashed_operators");

#[cw_serde]
pub struct Config {
    pub operator_contract: Addr,
    pub threshold_percent: Decimal,
    pub allowed_spread: Decimal,
    pub slashable_spread: Decimal,
    pub required_percentage: u32,
}

#[cw_serde]
pub struct PriceResult {
    pub price: Decimal,
}

#[cw_serde]
pub struct OperatorVote {
    pub power: Uint128,
    pub price: Decimal,
}
