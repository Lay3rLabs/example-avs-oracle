use std::str::FromStr;

use cosmwasm_std::Decimal;
use cw_orch::environment::{ChainState, CwEnv};
use cw_orch::prelude::*;

use lch_apis::tasks::{Requestor, Status, TimeoutInfo};
use lch_orch::{Addressable, AltSigner};
use serde_json::json;

use lavs_tasks::interface::Contract as TasksContract;
use lavs_tasks::msg::{
    CustomExecuteMsgFns as TasksExecuteMsgFns, CustomQueryMsgFns as TasksQueryMsgFns,
    InstantiateMsg as TasksInstantiateMsg,
};

use lavs_mock_voting::interface::Contract as MockOperatorsContract;
use lavs_mock_voting::msg::InstantiateMsg as MockOperatorsInstantiateMsg;

use crate::interface::Contract;
use crate::msg::{ExecuteMsgFns, InstantiateMsg, QueryMsgFns};

pub const BECH_PREFIX: &str = "slay3r";

pub fn setup<Chain: CwEnv>(chain: Chain, msg: InstantiateMsg) -> Contract<Chain> {
    let contract = Contract::new(chain);
    contract.upload().unwrap();
    contract.instantiate(&msg, None, None).unwrap();
    contract
}

pub fn happy_path<C>(chain: C)
where
    C: CwEnv + AltSigner,
    C::Sender: Addressable,
{
    // verifier setup
    let operator1 = chain.alt_signer(3);
    let operator2 = chain.alt_signer(4);
    let operator3 = chain.alt_signer(5);

    let operators = vec![
        (operator1.addr().to_string(), 50u64),
        (operator2.addr().to_string(), 30u64),
        (operator3.addr().to_string(), 20u64),
    ];
    let mock_operators = setup_mock_operators(chain.clone(), operators);

    let msg = InstantiateMsg {
        operator_contract: mock_operators.addr_str().unwrap(),
        // we want all our 3 operators to submit their votes
        threshold_percent: Decimal::percent(100),
        allowed_spread: Decimal::percent(10),
        slashable_spread: Decimal::percent(20),
    };
    let oracle_verifier = setup(chain.clone(), msg);

    // instantiate task queue
    let tasker = setup_task_queue(chain.clone(), &oracle_verifier.addr_str().unwrap());

    let payload = json!({"action": "get_price"});
    let task_id = make_task(&tasker, "Get Price Task", None, &payload);

    let result1 = r#"{"price": "100"}"#.to_string();
    oracle_verifier
        .call_as(&operator1)
        .executed_task(tasker.addr_str().unwrap(), task_id, result1)
        .unwrap();

    let result2 = r#"{"price": "102"}"#.to_string();
    oracle_verifier
        .call_as(&operator2)
        .executed_task(tasker.addr_str().unwrap(), task_id, result2)
        .unwrap();

    let result3 = r#"{"price": "98"}"#.to_string();
    oracle_verifier
        .call_as(&operator3)
        .executed_task(tasker.addr_str().unwrap(), task_id, result3)
        .unwrap();

    let status = tasker.task(task_id).unwrap();
    assert_eq!(
        status.status,
        Status::Completed {
            completed: chain.block_info().unwrap().time.seconds()
        }
    );

    let median_price = Decimal::from_str("100").unwrap();
    let task_result = status.result.unwrap();
    assert_eq!(task_result, json!({"price": median_price.to_string()}));

    let slashed_operators: Vec<Addr> = oracle_verifier.slashable_operators().unwrap();
    assert!(slashed_operators.is_empty());
}

pub fn threshold_not_met<C>(chain: C)
where
    C: CwEnv + AltSigner,
    C::Sender: Addressable,
{
    let operator1 = chain.alt_signer(3);
    let operator2 = chain.alt_signer(4);
    let operator3 = chain.alt_signer(5);

    let operators = vec![
        (operator1.addr().to_string(), 50u64),
        (operator2.addr().to_string(), 30u64),
        (operator3.addr().to_string(), 20u64),
    ];
    let mock_operators = setup_mock_operators(chain.clone(), operators);

    let msg = InstantiateMsg {
        operator_contract: mock_operators.addr_str().unwrap(),
        threshold_percent: Decimal::percent(90),
        allowed_spread: Decimal::percent(5),
        slashable_spread: Decimal::percent(10),
    };
    let verifier = setup(chain.clone(), msg);

    let tasker = setup_task_queue(chain.clone(), &verifier.addr_str().unwrap());

    let payload = json!({"action": "get_price"});
    let task_id = make_task(&tasker, "Get Price Task", None, &payload);

    let result = r#"{"price": "100"}"#.to_string();
    verifier
        .call_as(&operator1)
        .executed_task(tasker.addr_str().unwrap(), task_id, result.clone())
        .unwrap();

    let result = r#"{"price": "210"}"#.to_string();
    verifier
        .call_as(&operator2)
        .executed_task(tasker.addr_str().unwrap(), task_id, result.clone())
        .unwrap();

    let status = tasker.task(task_id).unwrap();

    assert_eq!(status.status, Status::Open {});
}

#[track_caller]
pub fn make_task<C: ChainState + TxHandler>(
    contract: &TasksContract<C>,
    name: &str,
    timeout: impl Into<Option<u64>>,
    payload: &serde_json::Value,
) -> u64 {
    let res = contract
        .create(name.to_string(), timeout.into(), payload.clone(), &[])
        .unwrap();
    get_task_id(&res)
}

#[track_caller]
pub fn get_task_id(res: &impl IndexResponse) -> u64 {
    res.event_attr_value("wasm", "task_id")
        .unwrap()
        .parse()
        .unwrap()
}

pub fn setup_task_queue<C>(chain: C, verifier_addr: &str) -> TasksContract<C>
where
    C: CwEnv + AltSigner,
    C::Sender: Addressable,
{
    let msg = TasksInstantiateMsg {
        requestor: Requestor::Fixed(chain.sender().into()),
        timeout: TimeoutInfo::new(600),
        verifier: verifier_addr.to_string(),
    };
    let tasker = TasksContract::new(chain);
    tasker.upload().unwrap();
    tasker.instantiate(&msg, None, None).unwrap();
    tasker
}

pub fn setup_mock_operators<C>(chain: C, operators: Vec<(String, u64)>) -> MockOperatorsContract<C>
where
    C: CwEnv + AltSigner,
    C::Sender: Addressable,
{
    let msg = MockOperatorsInstantiateMsg { operators };
    let mock_operators = MockOperatorsContract::new(chain);
    mock_operators.upload().unwrap();
    mock_operators.instantiate(&msg, None, None).unwrap();
    mock_operators
}
