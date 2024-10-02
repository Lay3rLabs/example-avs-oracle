use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{Addr, Decimal};
use cw_orch::ExecuteFns;
use lavs_apis::{id::TaskId, verifier_simple::OperatorVoteInfoResponse};

use crate::{state::Config, ContractError};

#[cw_serde]
pub struct InstantiateMsg {
    pub operator_contract: String,
    pub threshold_percent: Decimal,
    pub allowed_spread: Decimal,
    pub slashable_spread: Decimal,
}

#[cw_serde]
#[derive(ExecuteFns)]
#[cw_orch(disable_fields_sorting)]
pub enum ExecuteMsg {
    ExecutedTask {
        /// Task queue contract for which we completed the task
        task_queue_contract: String,
        /// The ID of the task that was completed
        task_id: TaskId,
        /// The result of the task, (JSON) serialized as a string
        /// It is serialized to allow for easy comparison and to avoid field sorting issues when verifying signatures
        result: String,
    },
}

#[cw_serde]
#[derive(cw_orch::QueryFns)]
#[cw_orch(disable_fields_sorting)]
#[derive(QueryResponses)]
pub enum QueryMsg {
    #[returns(Option<OperatorVoteInfoResponse>)]
    OperatorVote {
        /// The task contract we are interested in
        task_contract: String,
        /// The ID of the task we are interested in
        task_id: TaskId,
        /// The operator whose vote we are interested in
        operator: String,
    },
    #[returns(Vec<Addr>)]
    SlashableOperators {},
    #[returns(Config)]
    Config {},
}

impl InstantiateMsg {
    pub fn validate_percentages(&self) -> Result<(), ContractError> {
        let fields = [
            ("threshold_percent", &self.threshold_percent),
            ("allowed_spread", &self.allowed_spread),
            ("slashable_spread", &self.slashable_spread),
        ];

        // Check if any field has a value of zero or above 1 percent and return an error
        for (field_name, value) in fields.into_iter() {
            if *value == Decimal::zero() || value > &Decimal::percent(100) {
                return Err(ContractError::InvalidPercentage(
                    field_name.to_string(),
                    *value,
                ));
            }
        }

        if self.slashable_spread <= self.allowed_spread {
            return Err(ContractError::InvalidSpread(
                self.slashable_spread,
                self.allowed_spread,
            ));
        }

        Ok(())
    }
}
