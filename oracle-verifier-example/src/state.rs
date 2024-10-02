use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Decimal, Env, Uint128};
use cw_storage_plus::{Item, Map};
use lch_apis::tasks::TaskStatus;

pub const CONFIG: Item<Config> = Item::new("config");
pub const VOTES: Map<(&Addr, u64, &Addr), OperatorVote> = Map::new("operator_votes");
pub const TASKS: Map<(&Addr, u64), TaskMetadata> = Map::new("tasks");
pub const SLASHED_OPERATORS: Map<&Addr, bool> = Map::new("slashed_operators");

#[cw_serde]
pub struct Config {
    pub operators: Addr,
    pub threshold_percent: Decimal,
    pub allowed_spread: Decimal,
    pub slashable_spread: Decimal,
}

#[cw_serde]
pub struct PriceResult {
    pub price: Decimal,
}

#[cw_serde]
pub struct TaskMetadata {
    pub power_required: Uint128,
    pub status: TaskStatus,
    pub created_height: u64,
    /// Measured in UNIX seconds
    pub expires_time: u64,
}

impl TaskMetadata {
    pub fn is_expired(&self, env: &Env) -> bool {
        env.block.time.seconds() >= self.expires_time
    }
}

#[cw_serde]
pub struct OperatorVote {
    pub power: Uint128,
    pub price: Decimal,
}
