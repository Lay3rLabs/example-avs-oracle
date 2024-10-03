#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_json_binary, Addr, Binary, Deps, DepsMut, Env, MessageInfo, Order, Response, StdResult,
};
use cw2::set_contract_version;

use crate::error::ContractError;
use crate::msg::{ExecuteMsg, InstantiateMsg, QueryMsg};
use crate::state::{Config, CONFIG, SLASHED_OPERATORS, VOTES};

// version info for migration info
const CONTRACT_NAME: &str = env!("CARGO_PKG_NAME");
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    msg.validate_percentages()?;
    let op_addr = deps.api.addr_validate(&msg.operator_contract)?;
    let config = Config {
        operators: op_addr,
        threshold_percent: msg.threshold_percent,
        allowed_spread: msg.allowed_spread,
        slashable_spread: msg.slashable_spread,
        required_percentage: msg.required_percentage,
    };

    CONFIG.save(deps.storage, &config)?;

    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    Ok(Response::new())
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
pub fn query(deps: Deps, _env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::SlashableOperators {} => {
            let slashed_operators: Vec<Addr> = SLASHED_OPERATORS
                .keys(deps.storage, None, None, Order::Ascending)
                .collect::<StdResult<Vec<_>>>()?;
            to_json_binary(&slashed_operators)
        }
        QueryMsg::Config {} => {
            let config = CONFIG.load(deps.storage)?;
            to_json_binary(&config)
        }
        QueryMsg::OperatorVote {
            task_contract,
            task_id,
            operator,
        } => {
            let task_addr = deps.api.addr_validate(&task_contract)?;
            let operator_addr = deps.api.addr_validate(&operator)?;
            // Load the operator's vote for the given task
            let vote = VOTES.may_load(deps.storage, (&task_addr, task_id, &operator_addr))?;
            to_json_binary(&vote)
        }
    }
}

mod execute {
    use cosmwasm_std::{to_json_binary, Decimal, Order, Uint128, WasmMsg};
    use cw_utils::nonpayable;
    use lavs_apis::{
        id::TaskId,
        tasks::{TaskExecuteMsg, TaskStatus},
    };
    use lavs_helpers::verifier::ensure_valid_vote;
    use serde_json::from_str;

    use crate::state::{OperatorVote, PriceResult, SLASHED_OPERATORS, TASKS, VOTES};

    use super::*;

    pub fn executed_task(
        mut deps: DepsMut,
        env: Env,
        info: MessageInfo,
        task_queue_contract: String,
        task_id: TaskId,
        result: String,
    ) -> Result<Response, ContractError> {
        nonpayable(&info)?;

        // validate task and operator
        let task_queue = deps.api.addr_validate(&task_queue_contract)?;
        let operator = info.sender;

        let config = CONFIG.load(deps.storage)?;

        // operator allowed to vote and hasn't voted yet
        let (mut task_data, power) = match ensure_valid_vote(
            deps.branch(),
            &env,
            &task_queue,
            task_id,
            &operator,
            config.required_percentage,
            &config.operators,
        )? {
            Some(x) => x,
            None => return Ok(Response::default()),
        };

        let price_result: PriceResult = from_str(&result)?;
        if price_result.price.is_zero() {
            return Err(ContractError::ZeroPrice);
        }

        VOTES.save(
            deps.storage,
            (&task_queue, task_id, &operator),
            &OperatorVote {
                price: price_result.price,
                power,
            },
        )?;

        let all_votes: Vec<(Addr, OperatorVote)> = VOTES
            .prefix((&task_queue, task_id))
            .range(deps.storage, None, None, Order::Ascending)
            .collect::<StdResult<Vec<_>>>()?;

        let total_power: Uint128 = all_votes.iter().map(|(_, vote)| vote.power).sum();

        if total_power < task_data.power_required {
            return Ok(Response::new()
                .add_attribute("method", "executed_task")
                .add_attribute("status", "vote_stored"));
        }

        let config = CONFIG.load(deps.storage)?;

        let (median, slashable_operators, is_threshold_met) =
            process_votes(&all_votes, total_power, &config)?;

        let mut resp = Response::new();
        if is_threshold_met {
            for operator in slashable_operators {
                noop_slash_validator(&mut deps, &operator)?;
            }

            task_data.status = TaskStatus::Completed;
            TASKS.save(deps.storage, (&task_queue, task_id), &task_data)?;

            let response = serde_json::json!(PriceResult { price: median });

            let msg = WasmMsg::Execute {
                contract_addr: task_queue.to_string(),
                msg: to_json_binary(&TaskExecuteMsg::Complete { task_id, response })?,
                funds: vec![],
            };

            resp = resp
                .add_message(msg)
                .add_attribute("new_price", median.to_string());
        } else {
            resp = resp.add_attribute("status", "threshold_not_met");
        }

        resp = resp
            .add_attribute("method", "executed_task")
            .add_attribute("task_id", task_id.to_string())
            .add_attribute("task_queue_contract", task_queue_contract);

        // NOTE: If we ever want to optimize the storage:
        //let operator_keys: Vec<Addr> = VOTES
        //    .prefix((&task_queue, task_id))
        //    .keys(deps.storage, None, None, Order::Ascending)
        //    .collect::<StdResult<Vec<_>>>()?;
        //for operator in operator_keys {
        //    VOTES.remove(deps.storage, (&task_queue, task_id, &operator));
        //}

        Ok(resp)
    }

    pub(crate) fn calculate_median(values: &mut [Decimal]) -> Decimal {
        values.sort();

        if values.is_empty() {
            return Decimal::zero();
        }

        if values.len() % 2 == 0 {
            // first half                 + // second half              // divided by 2
            (values[values.len() / 2 - 1] + values[values.len() / 2]) / Uint128::new(2u128)
        } else {
            // take the middle value
            values[values.len() / 2]
        }
    }

    pub(crate) fn calculate_allowed_range(median: Decimal, spread: Decimal) -> (Decimal, Decimal) {
        let allowed_minimum = median * (Decimal::one() - spread);
        let allowed_maximum = median * (Decimal::one() + spread);
        (allowed_minimum, allowed_maximum)
    }

    pub(crate) fn filter_valid_votes(
        votes: &[(Addr, OperatorVote)],
        allowed_minimum: Decimal,
        allowed_maximum: Decimal,
    ) -> Vec<&(Addr, OperatorVote)> {
        votes
            .iter()
            .filter(|(_, vote)| vote.price >= allowed_minimum && vote.price <= allowed_maximum)
            .collect()
    }

    pub(crate) fn is_threshold_met(
        valid_power: Uint128,
        total_power: Uint128,
        threshold_percent: Decimal,
    ) -> bool {
        let valid_ratio = Decimal::from_ratio(valid_power, total_power);
        valid_ratio >= threshold_percent
    }

    pub(crate) fn identify_slashable_operators(
        votes: &[(Addr, OperatorVote)],
        slashable_minimum: Decimal,
        slashable_maximum: Decimal,
    ) -> Vec<Addr> {
        votes
            .iter()
            .filter_map(|(operator_addr, vote)| {
                let price = vote.price;
                if price < slashable_minimum || price > slashable_maximum {
                    Some(operator_addr.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    fn noop_slash_validator(deps: &mut DepsMut, operator: &Addr) -> Result<(), ContractError> {
        SLASHED_OPERATORS.save(deps.storage, operator, &true)?;
        //TODO: this should make an actual call to slash
        Ok(())
    }

    pub(crate) fn process_votes(
        votes: &[(Addr, OperatorVote)],
        total_power: Uint128,
        config: &Config,
    ) -> Result<(Decimal, Vec<Addr>, bool), ContractError> {
        let mut all_prices: Vec<Decimal> = votes.iter().map(|(_, vote)| vote.price).collect();

        let median = calculate_median(&mut all_prices);

        let (allowed_minimum, allowed_maximum) =
            calculate_allowed_range(median, config.allowed_spread);

        let valid_votes = filter_valid_votes(votes, allowed_minimum, allowed_maximum);

        let valid_power: Uint128 = valid_votes.iter().map(|(_, vote)| vote.power).sum();

        let is_threshold_met = is_threshold_met(valid_power, total_power, config.threshold_percent);

        let (slashable_minimum, slashable_maximum) =
            calculate_allowed_range(median, config.slashable_spread);

        let slashable_operators =
            identify_slashable_operators(votes, slashable_minimum, slashable_maximum);

        Ok((median, slashable_operators, is_threshold_met))
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use crate::state::OperatorVote;

    use super::*;
    use cosmwasm_std::{Decimal, Uint128};
    use execute::{
        calculate_allowed_range, calculate_median, filter_valid_votes,
        identify_slashable_operators, is_threshold_met, process_votes,
    };

    ////////////////////////////////////////////////
    /////////////// calculate_median ///////////////
    ////////////////////////////////////////////////

    #[test]
    fn calculate_median_odd_length() {
        let mut values = vec![Decimal::one(), Decimal::percent(300), Decimal::percent(500)];
        let median = calculate_median(&mut values);
        // we have 1, 3 and 5, so median should be 3
        assert_eq!(median, Decimal::percent(300));
    }

    #[test]
    fn calculate_median_even_length() {
        let mut values = vec![
            Decimal::one(),
            Decimal::percent(300),
            Decimal::percent(500),
            Decimal::percent(700),
        ];
        let median = calculate_median(&mut values);
        // this time we have 1, 3, 5 and 7 so median should be (3 + 5) / 2 = 4
        assert_eq!(median, Decimal::percent(400));
    }

    #[test]
    fn calculate_median_unsorted() {
        let mut values = vec![Decimal::percent(500), Decimal::one(), Decimal::percent(300)];
        let median = calculate_median(&mut values);
        // same as `calculate_median_odd_length` but unsorted
        assert_eq!(median, Decimal::percent(300));
    }

    #[test]
    fn calculate_median_single_element() {
        let mut values = vec![Decimal::percent(42)];
        let median = calculate_median(&mut values);
        assert_eq!(median, Decimal::percent(42));
    }

    #[test]
    fn calculate_median_fractional_values_odd() {
        let mut values = vec![
            Decimal::percent(11),
            Decimal::percent(12),
            Decimal::percent(13),
        ];
        let median = calculate_median(&mut values);
        // median should be 1.2
        assert_eq!(median, Decimal::percent(12));
    }

    #[test]
    fn calculate_median_fractional_values_even() {
        let mut values = vec![
            Decimal::percent(110),
            Decimal::percent(120),
            Decimal::percent(130),
            Decimal::percent(140),
        ];
        let median = calculate_median(&mut values);
        // (1.2 + 1.3) / 2 = 1.25
        assert_eq!(median, Decimal::percent(125));
    }

    #[test]
    fn calculate_median_identical_values() {
        let mut values = vec![
            Decimal::percent(500),
            Decimal::percent(500),
            Decimal::percent(500),
            Decimal::percent(500),
        ];
        let median = calculate_median(&mut values);
        assert_eq!(median, Decimal::percent(500));
    }

    #[test]
    fn calculate_median_large_numbers() {
        let mut values = vec![
            Decimal::percent(1_000_000_000_000u64),
            Decimal::percent(2_000_000_000_000u64),
            Decimal::percent(3_000_000_000_000u64),
        ];
        let median = calculate_median(&mut values);
        assert_eq!(median, Decimal::percent(2_000_000_000_000u64));
    }

    #[test]
    fn calculate_median_of_fib_unsorted() {
        let mut values = vec![
            Decimal::from_str("34").unwrap(),
            Decimal::from_str("2").unwrap(),
            Decimal::from_str("55").unwrap(),
            Decimal::from_str("5").unwrap(),
            Decimal::from_str("8").unwrap(),
            Decimal::from_str("13").unwrap(),
            Decimal::from_str("3").unwrap(),
            Decimal::from_str("21").unwrap(),
            Decimal::from_str("144").unwrap(),
            Decimal::from_str("1").unwrap(),
            Decimal::from_str("89").unwrap(),
            Decimal::from_str("8").unwrap(),
        ];

        // this will be sorted to
        // 1, 1, 2, 3, 5, 8, 13, 21, 34, 55, 89, 144
        // (8 + 13) / 2
        let median = calculate_median(&mut values);
        assert_eq!(median, Decimal::from_str("10.5").unwrap()) // 10.5
    }

    #[test]
    fn calculate_median_empty() {
        let mut values: Vec<Decimal> = vec![];
        let median = calculate_median(&mut values);
        assert_eq!(median, Decimal::zero())
    }

    ///////////////////////////////////////////////////////
    /////////////// calculate_allowed_range ///////////////
    ///////////////////////////////////////////////////////

    #[test]
    fn calculate_allowed_range_normal() {
        let median = Decimal::one();
        let spread = Decimal::percent(10);

        let (allowed_minimum, allowed_maximum) = calculate_allowed_range(median, spread);

        // allowed_minimum = 100 * (1 - 0.10) = 90
        // allowed_maximum = 100 * (1 + 0.10) = 110

        assert_eq!(allowed_minimum, Decimal::percent(90));
        assert_eq!(allowed_maximum, Decimal::percent(110));
    }

    #[test]
    fn calculate_allowed_range_zero_spread() {
        let median = Decimal::one();
        let spread = Decimal::zero();

        let (allowed_minimum, allowed_maximum) = calculate_allowed_range(median, spread);

        // allowed_minimum = 100 * (1 - 0) = 100
        // allowed_maximum = 100 * (1 + 0) = 100

        assert_eq!(allowed_minimum, median);
        assert_eq!(allowed_maximum, median);
    }

    #[test]
    fn calculate_allowed_range_full_spread() {
        let median = Decimal::one();
        let spread = Decimal::one();

        let (allowed_minimum, allowed_maximum) = calculate_allowed_range(median, spread);

        // allowed_minimum = 100 * (1 - 1) = 0
        // allowed_maximum = 100 * (1 + 1) = 200

        assert_eq!(allowed_minimum, Decimal::zero());
        assert_eq!(allowed_maximum, Decimal::from_str("2").unwrap());
    }

    #[test]
    fn calculate_allowed_range_zero_median() {
        let median = Decimal::zero();
        let spread = Decimal::percent(10);

        let (allowed_minimum, allowed_maximum) = calculate_allowed_range(median, spread);

        assert_eq!(allowed_minimum, Decimal::zero());
        assert_eq!(allowed_maximum, Decimal::zero());
    }

    #[test]
    fn calculate_allowed_range_fractional_median() {
        let median = Decimal::from_str("15").unwrap();
        let spread = Decimal::from_str("0.1").unwrap();

        let (allowed_minimum, allowed_maximum) = calculate_allowed_range(median, spread);

        // allowed_minimum = 15.0 * (1 - 0.10) = 13.5
        // allowed_maximum = 15.0 * (1 + 0.10) = 16.5

        assert_eq!(allowed_minimum, Decimal::from_str("13.5").unwrap()); // 13.5
        assert_eq!(allowed_maximum, Decimal::from_str("16.5").unwrap()); // 16.5
    }

    #[test]
    fn calculate_allowed_range_fractional_spread() {
        let median = Decimal::one();
        // 0.15 or %15
        let spread = Decimal::percent(15);

        let (allowed_minimum, allowed_maximum) = calculate_allowed_range(median, spread);

        // allowed_minimum = 100 * (1 - 0.15) = 85
        // allowed_maximum = 100 * (1 + 0.15) = 115

        assert_eq!(allowed_minimum, Decimal::percent(85));
        assert_eq!(allowed_maximum, Decimal::percent(115));
    }

    #[test]
    fn calculate_allowed_range_large_numbers() {
        let median = Decimal::percent(1_000_000_000_000u64);
        let spread = Decimal::percent(10);

        let (allowed_minimum, allowed_maximum) = calculate_allowed_range(median, spread);

        // allowed_minimum = 1,000,000,000,000 * (1 - 0.1) = 900,000,000,000
        // allowed_maximum = 1,000,000,000,000 * (1 + 0.1) = 1,100,000,000,000

        assert_eq!(allowed_minimum, Decimal::percent(900_000_000_000u64));
        assert_eq!(allowed_maximum, Decimal::percent(1_100_000_000_000u64));
    }

    //////////////////////////////////////////////////
    /////////////// filter_valid_votes ///////////////
    //////////////////////////////////////////////////

    #[test]
    fn filter_within_bounds() {
        let op1 = Addr::unchecked("addr1");
        let op2 = Addr::unchecked("addr2");
        let op3 = Addr::unchecked("addr3");

        let vote1 = OperatorVote {
            power: Uint128::new(100),
            price: Decimal::from_str("1.5").unwrap(),
        };
        let vote2 = OperatorVote {
            power: Uint128::new(200),
            price: Decimal::from_str("2.0").unwrap(),
        };
        let vote3 = OperatorVote {
            power: Uint128::new(300),
            price: Decimal::from_str("2.5").unwrap(),
        };

        let votes = vec![
            (op1.clone(), vote1),
            (op2.clone(), vote2),
            (op3.clone(), vote3),
        ];

        // Allowed ranges
        let min_price = Decimal::from_str("1.5").unwrap();
        let max_price = Decimal::from_str("2.5").unwrap();

        let result = filter_valid_votes(&votes, min_price, max_price);

        assert_eq!(result.len(), 3);
        assert_eq!(
            result[0],
            &(
                op1,
                OperatorVote {
                    power: Uint128::new(100),
                    price: Decimal::from_str("1.5").unwrap()
                }
            )
        );
        assert_eq!(
            result[1],
            &(
                op2,
                OperatorVote {
                    power: Uint128::new(200),
                    price: Decimal::from_str("2.0").unwrap()
                }
            )
        );
        assert_eq!(
            result[2],
            &(
                op3,
                OperatorVote {
                    power: Uint128::new(300),
                    price: Decimal::from_str("2.5").unwrap()
                }
            )
        );
    }

    #[test]
    fn filter_out_of_bounds() {
        let op1 = Addr::unchecked("addr1");
        let op2 = Addr::unchecked("addr2");
        let op3 = Addr::unchecked("addr3");

        let vote1 = OperatorVote {
            power: Uint128::new(100),
            price: Decimal::from_str("1.0").unwrap(),
        };
        let vote2 = OperatorVote {
            power: Uint128::new(200),
            price: Decimal::from_str("2.0").unwrap(),
        };
        let vote3 = OperatorVote {
            power: Uint128::new(300),
            price: Decimal::from_str("3.0").unwrap(),
        };

        let votes = vec![
            (op1.clone(), vote1),
            (op2.clone(), vote2),
            (op3.clone(), vote3),
        ];

        let min_price = Decimal::from_str("1.5").unwrap();
        let max_price = Decimal::from_str("2.5").unwrap();

        let result = filter_valid_votes(&votes, min_price, max_price);

        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0],
            &(
                op2,
                OperatorVote {
                    power: Uint128::new(200),
                    price: Decimal::from_str("2.0").unwrap()
                }
            )
        );
    }

    #[test]
    fn filter_all_out_of_bounds() {
        let op1 = Addr::unchecked("addr1");
        let op2 = Addr::unchecked("addr2");
        let op3 = Addr::unchecked("addr3");

        let vote1 = OperatorVote {
            power: Uint128::new(100),
            price: Decimal::from_str("0.5").unwrap(),
        };
        let vote2 = OperatorVote {
            power: Uint128::new(200),
            price: Decimal::from_str("4.0").unwrap(),
        };
        let vote3 = OperatorVote {
            power: Uint128::new(300),
            price: Decimal::from_str("5.0").unwrap(),
        };

        let votes = vec![
            (op1.clone(), vote1),
            (op2.clone(), vote2),
            (op3.clone(), vote3),
        ];

        let min_price = Decimal::from_str("1.5").unwrap();
        let max_price = Decimal::from_str("2.5").unwrap();

        let result = filter_valid_votes(&votes, min_price, max_price);

        assert_eq!(result.len(), 0);
    }

    #[test]
    fn filter_edge_cases() {
        let op1 = Addr::unchecked("addr1");
        let op2 = Addr::unchecked("addr2");

        let vote1 = OperatorVote {
            power: Uint128::new(100),
            price: Decimal::from_str("1.5").unwrap(),
        };
        let vote2 = OperatorVote {
            power: Uint128::new(200),
            price: Decimal::from_str("2.5").unwrap(),
        };

        let votes = vec![(op1.clone(), vote1), (op2.clone(), vote2)];

        let min_price = Decimal::from_str("1.5").unwrap();
        let max_price = Decimal::from_str("2.5").unwrap();

        let result = filter_valid_votes(&votes, min_price, max_price);

        assert_eq!(result.len(), 2);
        assert_eq!(
            result[0],
            &(
                op1,
                OperatorVote {
                    power: Uint128::new(100),
                    price: Decimal::from_str("1.5").unwrap()
                }
            )
        );
        assert_eq!(
            result[1],
            &(
                op2,
                OperatorVote {
                    power: Uint128::new(200),
                    price: Decimal::from_str("2.5").unwrap()
                }
            )
        );
    }

    ////////////////////////////////////////////////
    /////////////// is_threshold_met ///////////////
    ////////////////////////////////////////////////

    #[test]
    fn threshold_met_exact() {
        let valid_power = Uint128::new(50);
        let total_power = Uint128::new(100);
        let threshold_percent = Decimal::percent(50);

        let result = is_threshold_met(valid_power, total_power, threshold_percent);
        assert!(
            result,
            "threshold should be met when valid is %50 of total power"
        );
    }

    #[test]
    fn threshold_not_met() {
        let valid_power = Uint128::new(40);
        let total_power = Uint128::new(100);
        let threshold_percent = Decimal::percent(50);

        let result = is_threshold_met(valid_power, total_power, threshold_percent);
        assert!(!result, "threshold should be not met when not enough power");
    }

    #[test]
    fn threshold_exceeded() {
        let valid_power = Uint128::new(60);
        let total_power = Uint128::new(100);
        let threshold_percent = Decimal::percent(50);

        let result = is_threshold_met(valid_power, total_power, threshold_percent);
        assert!(result, "should return true when threshold met over %50");
    }

    #[test]
    fn full_power_threshold() {
        let valid_power = Uint128::new(100);
        let total_power = Uint128::new(100);
        let threshold_percent = Decimal::percent(100);

        let result = is_threshold_met(valid_power, total_power, threshold_percent);
        assert!(
            result,
            "should return true when valid power is equal total power"
        );
    }

    #[test]
    fn threshold_met_minimum_case() {
        let valid_power = Uint128::new(2);
        let total_power = Uint128::new(100);
        let threshold_percent = Decimal::percent(1);

        let result = is_threshold_met(valid_power, total_power, threshold_percent);
        assert!(
            result,
            "should return true when valid power is over the threshold"
        );
    }

    ////////////////////////////////////////////////////////////
    /////////////// identify_slashable_operators ///////////////
    ////////////////////////////////////////////////////////////

    #[test]
    fn no_slashable_operators() {
        let op1 = Addr::unchecked("operator1");
        let op2 = Addr::unchecked("operator2");
        let op3 = Addr::unchecked("operator3");

        let vote1 = OperatorVote {
            power: Uint128::new(100),
            price: Decimal::percent(150),
        };
        let vote2 = OperatorVote {
            power: Uint128::new(200),
            price: Decimal::percent(200),
        };
        let vote3 = OperatorVote {
            power: Uint128::new(300),
            price: Decimal::percent(250),
        };

        let votes = vec![
            (op1.clone(), vote1),
            (op2.clone(), vote2),
            (op3.clone(), vote3),
        ];

        let slashable_minimum = Decimal::percent(150);
        let slashable_maximum = Decimal::percent(250);

        let result = identify_slashable_operators(&votes, slashable_minimum, slashable_maximum);
        assert_eq!(result.len(), 0, "there should be no slashable operators");
    }

    #[test]
    fn some_slashable_operators() {
        let op1 = Addr::unchecked("operator1");
        let op2 = Addr::unchecked("operator2");
        let op3 = Addr::unchecked("operator3");

        let vote1 = OperatorVote {
            power: Uint128::new(100),
            price: Decimal::percent(100),
        };
        let vote2 = OperatorVote {
            power: Uint128::new(200),
            price: Decimal::percent(200),
        };
        let vote3 = OperatorVote {
            power: Uint128::new(300),
            price: Decimal::percent(300),
        };

        let votes = vec![
            (op1.clone(), vote1),
            (op2.clone(), vote2),
            (op3.clone(), vote3),
        ];

        let slashable_minimum = Decimal::percent(150);
        let slashable_maximum = Decimal::percent(250);

        let result = identify_slashable_operators(&votes, slashable_minimum, slashable_maximum);
        assert_eq!(result.len(), 2, "we must have 2 slashable operators");
        assert_eq!(result, vec![op1.clone(), op3.clone()]);
    }

    #[test]
    fn all_slashable_operators() {
        let op1 = Addr::unchecked("operator1");
        let op2 = Addr::unchecked("operator2");
        let op3 = Addr::unchecked("operator3");

        let vote1 = OperatorVote {
            power: Uint128::new(100),
            price: Decimal::percent(50),
        };
        let vote2 = OperatorVote {
            power: Uint128::new(200),
            price: Decimal::percent(300),
        };
        let vote3 = OperatorVote {
            power: Uint128::new(300),
            price: Decimal::percent(400),
        };

        let votes = vec![
            (op1.clone(), vote1),
            (op2.clone(), vote2),
            (op3.clone(), vote3),
        ];

        let slashable_minimum = Decimal::percent(150);
        let slashable_maximum = Decimal::percent(250);

        let result = identify_slashable_operators(&votes, slashable_minimum, slashable_maximum);
        assert_eq!(result.len(), 3, "all operators should be slashed");
        assert_eq!(result, vec![op1.clone(), op2.clone(), op3.clone()]);
    }

    #[test]
    fn edge_case_slashable_operators() {
        let op1 = Addr::unchecked("operator1");
        let op2 = Addr::unchecked("operator2");

        let vote1 = OperatorVote {
            power: Uint128::new(100),
            // low blound
            price: Decimal::percent(150),
        };
        let vote2 = OperatorVote {
            power: Uint128::new(200),
            // upper bound
            price: Decimal::percent(250),
        };

        let votes = vec![(op1.clone(), vote1), (op2.clone(), vote2)];

        let slashable_minimum = Decimal::percent(150);
        let slashable_maximum = Decimal::percent(250);

        let result = identify_slashable_operators(&votes, slashable_minimum, slashable_maximum);
        assert_eq!(result.len(), 0, "operators shouldn't be slashed");
    }

    #[test]
    fn empty_votes() {
        let votes: Vec<(Addr, OperatorVote)> = vec![];

        let slashable_minimum = Decimal::from_str("1.5").unwrap();
        let slashable_maximum = Decimal::from_str("2.5").unwrap();

        let result = identify_slashable_operators(&votes, slashable_minimum, slashable_maximum);
        assert_eq!(result.len(), 0, "there should be none from an empty list");
    }

    /////////////////////////////////////////////
    /////////////// process_votes ///////////////
    /////////////////////////////////////////////

    #[test]
    fn process_votes_meets_threshold() {
        let op1 = Addr::unchecked("operator1");
        let op2 = Addr::unchecked("operator2");

        let votes = vec![
            (
                op1.clone(),
                OperatorVote {
                    power: Uint128::new(100),
                    price: Decimal::from_str("1.0").unwrap(),
                },
            ),
            (
                op2.clone(),
                OperatorVote {
                    power: Uint128::new(100),
                    price: Decimal::from_str("1.0").unwrap(),
                },
            ),
        ];

        let config = Config {
            operators: Addr::unchecked("operators"),
            threshold_percent: Decimal::percent(50),
            allowed_spread: Decimal::percent(10),
            slashable_spread: Decimal::percent(20),
            required_percentage: 70,
        };

        // mocking the power
        let result = process_votes(&votes, Uint128::new(100), &config).unwrap();

        let expected_median = Decimal::percent(100);
        let expected_slashable_operators: Vec<Addr> = vec![];
        let expected_is_threshold_met = true;

        assert_eq!(result.0, expected_median);
        assert_eq!(result.1, expected_slashable_operators);
        assert_eq!(result.2, expected_is_threshold_met);
    }

    #[test]
    fn process_votes_threshold_not_met() {
        let op1 = Addr::unchecked("operator1");
        let op2 = Addr::unchecked("operator2");

        let votes = vec![
            (
                op1.clone(),
                OperatorVote {
                    power: Uint128::new(20),
                    price: Decimal::from_str("1.0").unwrap(),
                },
            ),
            (
                op2.clone(),
                OperatorVote {
                    power: Uint128::new(90),
                    price: Decimal::from_str("3.0").unwrap(),
                },
            ),
        ];

        let config = Config {
            operators: Addr::unchecked("operators"),
            threshold_percent: Decimal::percent(80),
            allowed_spread: Decimal::percent(10),
            slashable_spread: Decimal::percent(20),
            required_percentage: 70,
        };

        // mocking the power
        let result = process_votes(&votes, Uint128::new(100), &config).unwrap();

        let expected_median = Decimal::from_str("2.0").unwrap();
        let expected_slashable_operators = vec![op1.clone(), op2.clone()];
        let expected_is_threshold_met = false;

        assert_eq!(result.0, expected_median);
        assert_eq!(result.1, expected_slashable_operators);
        assert_eq!(result.2, expected_is_threshold_met);
    }

    #[test]
    fn test_process_votes_slashable_operators() {
        let op1 = Addr::unchecked("operator1");
        let op2 = Addr::unchecked("operator2");
        let op3 = Addr::unchecked("operator3");

        let votes = vec![
            (
                op1.clone(),
                OperatorVote {
                    power: Uint128::new(50),
                    price: Decimal::from_str("1.5").unwrap(),
                },
            ),
            (
                op2.clone(),
                OperatorVote {
                    power: Uint128::new(50),
                    price: Decimal::from_str("2.0").unwrap(),
                },
            ),
            (
                op3.clone(),
                OperatorVote {
                    power: Uint128::new(50),
                    price: Decimal::from_str("3.5").unwrap(),
                },
            ),
        ];

        let config = Config {
            operators: Addr::unchecked("operators"),
            threshold_percent: Decimal::from_str("0.33").unwrap(),
            allowed_spread: Decimal::from_str("0.1").unwrap(),
            slashable_spread: Decimal::from_str("0.2").unwrap(),
            required_percentage: 70,
        };

        // mocking the power
        let result = process_votes(&votes, Uint128::new(100), &config).unwrap();

        let expected_median = Decimal::from_str("2.0").unwrap();
        let expected_slashable_operators = vec![op1.clone(), op3.clone()];
        let expected_is_threshold_met = true;

        assert_eq!(result.0, expected_median);
        assert_eq!(result.1, expected_slashable_operators);
        assert_eq!(result.2, expected_is_threshold_met);
    }

    #[test]
    fn test_process_votes_insufficient_power_votes() {
        let operator1 = Addr::unchecked("operator1");
        let operator2 = Addr::unchecked("operator2");
        let operator3 = Addr::unchecked("operator3");

        let total_power = Uint128::new(100);

        let config = Config {
            operators: Addr::unchecked("operator_contract"),
            threshold_percent: Decimal::percent(50),
            allowed_spread: Decimal::percent(10),
            slashable_spread: Decimal::percent(20),
            required_percentage: 70,
        };

        let votes = vec![
            (
                operator1.clone(),
                OperatorVote {
                    price: Decimal::from_str("100").unwrap(),
                    power: Uint128::new(20),
                },
            ),
            (
                operator2.clone(),
                OperatorVote {
                    price: Decimal::from_str("102").unwrap(),
                    power: Uint128::new(20),
                },
            ),
        ];

        let (median, slashed_operators, is_threshold_met) =
            process_votes(&votes, total_power, &config).unwrap();

        assert!(!is_threshold_met);
        assert_eq!(median, Decimal::from_str("101").unwrap());
        assert_eq!(slashed_operators.len(), 0);

        let votes_with_op3 = vec![
            votes[0].clone(),
            votes[1].clone(),
            (
                operator3.clone(),
                OperatorVote {
                    price: Decimal::from_str("98").unwrap(),
                    power: Uint128::new(60),
                },
            ),
        ];

        let (median, slashed_operators, is_threshold_met) =
            process_votes(&votes_with_op3, total_power, &config).unwrap();

        assert!(is_threshold_met);
        // NOTE: This would have ot change once the weighted calculation of votes is in place
        assert_eq!(median, Decimal::from_str("100").unwrap());
        assert_eq!(slashed_operators.len(), 0);
    }

    #[test]
    fn test_process_votes_spread_exceeds_allowed() {
        let operator1 = Addr::unchecked("operator1");
        let operator2 = Addr::unchecked("operator2");
        let operator3 = Addr::unchecked("operator3");

        let total_power = Uint128::new(100);

        let config = Config {
            operators: Addr::unchecked("operator_contract"),
            threshold_percent: Decimal::percent(100),
            allowed_spread: Decimal::percent(10),
            slashable_spread: Decimal::percent(20),
            required_percentage: 70,
        };

        let votes = vec![
            (
                operator1.clone(),
                OperatorVote {
                    price: Decimal::from_str("100").unwrap(),
                    power: Uint128::new(50),
                },
            ),
            (
                operator2.clone(),
                OperatorVote {
                    price: Decimal::from_str("130").unwrap(),
                    power: Uint128::new(30),
                },
            ),
            (
                operator3.clone(),
                OperatorVote {
                    price: Decimal::from_str("70").unwrap(),
                    power: Uint128::new(20),
                },
            ),
        ];

        let (median, slashed_operators, is_threshold_met) =
            process_votes(&votes, total_power, &config).unwrap();

        assert!(!is_threshold_met);
        // NOTE: This would have ot change once the weighted calculation of votes is in place
        assert_eq!(median, Decimal::from_str("100").unwrap());
        assert_eq!(slashed_operators.len(), 2);
    }

    #[test]
    fn test_process_votes_one_operator_slashed() {
        let operator1 = Addr::unchecked("operator1");
        let operator2 = Addr::unchecked("operator2");
        let operator3 = Addr::unchecked("operator3");

        let total_power = Uint128::new(100);

        let config = Config {
            operators: Addr::unchecked("operator_contract"),
            threshold_percent: Decimal::percent(80),
            allowed_spread: Decimal::percent(10),
            slashable_spread: Decimal::percent(20),
            required_percentage: 70,
        };

        let votes = vec![
            (
                operator1.clone(),
                OperatorVote {
                    price: Decimal::from_str("100").unwrap(),
                    power: Uint128::new(50),
                },
            ),
            (
                operator2.clone(),
                OperatorVote {
                    price: Decimal::from_str("105").unwrap(),
                    power: Uint128::new(30),
                },
            ),
            (
                operator3.clone(),
                OperatorVote {
                    price: Decimal::from_str("150").unwrap(), // Outlier
                    power: Uint128::new(20),
                },
            ),
        ];

        let (median, slashed_operators, is_threshold_met) =
            process_votes(&votes, total_power, &config).unwrap();

        assert!(is_threshold_met);
        //NOTE: This will change with weighted median calculation
        assert_eq!(median, Decimal::from_str("105").unwrap());
        assert_eq!(slashed_operators, vec![operator3.clone()]);
    }

    #[test]
    fn test_process_votes_median_calculation() {
        let operator1 = Addr::unchecked("operator1");
        let operator2 = Addr::unchecked("operator2");
        let operator3 = Addr::unchecked("operator3");

        let total_power = Uint128::new(100);

        let config = Config {
            operators: Addr::unchecked("operator_contract"),
            threshold_percent: Decimal::percent(100),
            allowed_spread: Decimal::percent(50),
            slashable_spread: Decimal::percent(60),
            required_percentage: 70,
        };

        let votes = vec![
            (
                operator1.clone(),
                OperatorVote {
                    price: Decimal::from_str("100").unwrap(),
                    power: Uint128::new(50),
                },
            ),
            (
                operator2.clone(),
                OperatorVote {
                    price: Decimal::from_str("110").unwrap(),
                    power: Uint128::new(30),
                },
            ),
            (
                operator3.clone(),
                OperatorVote {
                    price: Decimal::from_str("120").unwrap(),
                    power: Uint128::new(20),
                },
            ),
        ];

        let (median, slashed_operators, is_threshold_met) =
            process_votes(&votes, total_power, &config).unwrap();

        assert_eq!(median, Decimal::from_str("110").unwrap());
        assert!(is_threshold_met);
        assert_eq!(slashed_operators.len(), 0);
    }
}
