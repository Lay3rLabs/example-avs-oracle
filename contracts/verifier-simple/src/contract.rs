#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{to_json_binary, Binary, Deps, DepsMut, Env, MessageInfo, Response, StdResult};
use cw2::set_contract_version;

use crate::error::ContractError;
use crate::msg::{ExecuteMsg, InstantiateMsg, QueryMsg};

use crate::state::{Config, CONFIG};

// version info for migration info
const CONTRACT_NAME: &str = "crates.io:lavs-verifier-simple";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    // validate the input data
    let operators = deps.api.addr_validate(&msg.operator_contract)?;
    let required_percentage = msg.required_percentage;
    if required_percentage > 100 || required_percentage == 0 {
        return Err(ContractError::InvalidPercentage);
    }

    // save config and cw2 metadata
    let config = Config {
        operators,
        required_percentage,
    };
    CONFIG.save(deps.storage, &config)?;
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    Ok(Response::default())
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        ExecuteMsg::ExecutedTask {
            task_queue_contract,
            task_id,
            result,
        } => execute::executed_task(deps, env, info, task_queue_contract, task_id, result),
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Config {} => Ok(to_json_binary(&query::config(deps)?)?),
        QueryMsg::TaskInfo {
            task_contract,
            task_id,
        } => Ok(to_json_binary(&query::task_info(
            deps,
            env,
            task_contract,
            task_id,
        )?)?),
        QueryMsg::OperatorVote {
            task_contract,
            task_id,
            operator,
        } => Ok(to_json_binary(&query::operator_vote(
            deps,
            task_contract,
            task_id,
            operator,
        )?)?),
    }
}

mod execute {
    use super::*;

    use cosmwasm_std::{from_json, Addr, Decimal, Uint128, WasmMsg};

    use cw_utils::nonpayable;
    use lavs_apis::interfaces::tasks::{
        ResponseType, TaskExecuteMsg, TaskQueryMsg, TaskStatus, TaskStatusResponse,
    };
    use lavs_apis::interfaces::voting::{
        QueryMsg as OperatorQueryMsg, TotalPowerResponse, VotingPowerResponse,
    };

    use crate::state::{record_vote, TaskMetadata, TASKS, VOTES};

    pub fn executed_task(
        mut deps: DepsMut,
        env: Env,
        info: MessageInfo,
        task_queue_contract: String,
        task_id: u64,
        result: String,
    ) -> Result<Response, ContractError> {
        nonpayable(&info)?;

        // Ensure task is open and this operator can vote
        let task_queue = deps.api.addr_validate(&task_queue_contract)?;
        let operator = info.sender;

        // verify the result type upon submissions (parse it into expected ResponseType)
        let _: ResponseType = from_json(&result)?;

        // Verify this operator is allowed to vote and has not voted yet, and do some initialization
        let (mut task_data, power) =
            match ensure_valid_vote(deps.branch(), &env, &task_queue, task_id, &operator)? {
                Some(x) => x,
                None => return Ok(Response::default()),
            };

        // Update the vote and check the total power on this result, also recording the operators vote
        let tally = record_vote(
            deps.storage,
            &task_queue,
            task_id,
            &operator,
            &result,
            power,
        )?;

        // Create the result with standard attributes
        let mut res = Response::new()
            .add_attribute("action", "execute")
            .add_attribute("task_id", task_id.to_string())
            .add_attribute("task_queue", &task_queue_contract)
            .add_attribute("operator", operator);

        // If there is enough power, let's submit it as completed
        // We add completed attribute to mark if this was the last one or not
        if tally >= task_data.power_required {
            // We need to update the status as completed
            task_data.status = TaskStatus::Completed;
            TASKS.save(deps.storage, (&task_queue, task_id), &task_data)?;

            // And submit the result to the task queue (after parsing it into relevant type)
            let response: ResponseType = from_json(&result)?;
            res = res
                .add_message(WasmMsg::Execute {
                    contract_addr: task_queue_contract,
                    msg: to_json_binary(&TaskExecuteMsg::Complete { task_id, response })?,
                    funds: vec![],
                })
                .add_attribute("completed", "true");
        } else {
            res = res.add_attribute("completed", "false");
        }

        Ok(res)
    }

    /// Does all checks to ensure the voter is valid and has not voted yet.
    /// Also checks the task is valid and still open.
    /// Returns the metadata for the task (creating it if first voter), along with the voting power of this operator.
    ///
    /// We do not want to error if an operator votes for a task that is already completed (due to race conditions).
    /// In that case, just return None and exit early rather than error.
    fn ensure_valid_vote(
        mut deps: DepsMut,
        env: &Env,
        task_queue: &Addr,
        task_id: u64,
        operator: &Addr,
    ) -> Result<Option<(TaskMetadata, Uint128)>, ContractError> {
        // Operator has not submitted a vote yet
        let vote = VOTES.may_load(deps.storage, (task_queue, task_id, operator))?;
        if vote.is_some() {
            return Err(ContractError::OperatorAlreadyVoted(operator.to_string()));
        }

        // get config for future queries
        let config = CONFIG.load(deps.storage)?;

        // Load task info, or create it if not there
        // Error here means the contract is in expired or completed, return None rather than error
        let metadata =
            match load_or_initialize_metadata(deps.branch(), env, &config, task_queue, task_id) {
                Ok(x) => x,
                Err(_) => return Ok(None),
            };

        // Get the operators voting power at time of vote creation
        let power: VotingPowerResponse = deps.querier.query_wasm_smart(
            config.operators.to_string(),
            &OperatorQueryMsg::VotingPowerAtHeight {
                address: operator.to_string(),
                height: Some(metadata.created_height),
            },
        )?;
        if power.power.is_zero() {
            return Err(ContractError::Unauthorized);
        }

        Ok(Some((metadata, power.power)))
    }

    fn load_or_initialize_metadata(
        deps: DepsMut,
        env: &Env,
        config: &Config,
        task_queue: &Addr,
        task_id: u64,
    ) -> Result<TaskMetadata, ContractError> {
        let metadata = TASKS.may_load(deps.storage, (task_queue, task_id))?;
        match metadata {
            Some(meta) => {
                // Ensure this is not yet expired (or completed)
                match meta.status {
                    TaskStatus::Completed => Err(ContractError::TaskAlreadyCompleted),
                    TaskStatus::Expired => Err(ContractError::TaskExpired),
                    TaskStatus::Open if meta.is_expired(env) => Err(ContractError::TaskExpired),
                    _ => Ok(meta),
                }
            }
            None => {
                // We need to query the info from the task queue
                let task_status: TaskStatusResponse = deps.querier.query_wasm_smart(
                    task_queue.to_string(),
                    &TaskQueryMsg::TaskStatus { id: task_id },
                )?;
                // Abort early if not still open
                match task_status.status {
                    TaskStatus::Completed => Err(ContractError::TaskAlreadyCompleted),
                    TaskStatus::Expired => Err(ContractError::TaskExpired),
                    TaskStatus::Open => {
                        // If we create this, we need to calculate total vote power needed
                        let total_power: TotalPowerResponse = deps.querier.query_wasm_smart(
                            config.operators.to_string(),
                            &OperatorQueryMsg::TotalPowerAtHeight {
                                height: Some(task_status.created_height),
                            },
                        )?;
                        // need to round up!
                        let fraction = Decimal::percent(config.required_percentage as u64);
                        let power_required = total_power.power.mul_ceil(fraction);
                        let meta = TaskMetadata {
                            power_required,
                            status: TaskStatus::Open,
                            created_height: task_status.created_height,
                            expires_time: task_status.expires_time,
                        };
                        TASKS.save(deps.storage, (task_queue, task_id), &meta)?;
                        Ok(meta)
                    }
                }
            }
        }
    }
}

mod query {
    use lavs_apis::verifier_simple::{TaskStatus, TaskTally};

    use super::*;

    use crate::msg::{ConfigResponse, OperatorVoteInfoResponse, TaskInfoResponse};
    use crate::state::{OPTIONS, TASKS, VOTES};

    pub fn config(deps: Deps) -> StdResult<ConfigResponse> {
        let cfg = CONFIG.load(deps.storage)?;
        Ok(ConfigResponse {
            operator_contract: cfg.operators.to_string(),
            required_percentage: cfg.required_percentage,
        })
    }

    pub fn task_info(
        deps: Deps,
        env: Env,
        task_contract: String,
        task_id: u64,
    ) -> StdResult<Option<TaskInfoResponse>> {
        let task_contract = deps.api.addr_validate(&task_contract)?;
        let info = TASKS.may_load(deps.storage, (&task_contract, task_id))?;
        if let Some(i) = info {
            // Check current time and update the status if it expired
            let status = match i.status {
                TaskStatus::Open if i.is_expired(&env) => TaskStatus::Expired,
                x => x,
            };
            // Collect the running tallies on the options
            let tallies: Result<Vec<_>, _> = OPTIONS
                .range(deps.storage, None, None, cosmwasm_std::Order::Ascending)
                .map(|r| {
                    r.map(|((_, _, result), v)| TaskTally {
                        result,
                        power: v.power,
                    })
                })
                .collect();
            let res = TaskInfoResponse {
                status,
                power_needed: i.power_required,
                tallies: tallies?,
            };
            Ok(Some(res))
        } else {
            Ok(None)
        }
    }

    pub fn operator_vote(
        deps: Deps,
        task_contract: String,
        task_id: u64,
        operator: String,
    ) -> StdResult<Option<OperatorVoteInfoResponse>> {
        let task_contract = deps.api.addr_validate(&task_contract)?;
        let operator = deps.api.addr_validate(&operator)?;
        let vote = VOTES
            .may_load(deps.storage, (&task_contract, task_id, &operator))?
            .map(|v| OperatorVoteInfoResponse {
                power: v.power,
                result: v.result,
            });
        Ok(vote)
    }
}

#[cfg(test)]
mod tests {}
