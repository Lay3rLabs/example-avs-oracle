use cosmwasm_std::Uint128;
use cw_orch::environment::CwEnv;
use cw_orch::prelude::*;

use lavs_orch::{Addressable, AltSigner};

use crate::interface::Contract;
use crate::msg::{InstantiateMsg, QueryMsgFns};

pub const BECH_PREFIX: &str = "slay3r";

pub fn setup<Chain: CwEnv>(chain: Chain, msg: InstantiateMsg) -> Contract<Chain> {
    let contract = Contract::new(chain);
    contract.upload().unwrap();
    contract.instantiate(&msg, None, None).unwrap();
    contract
}

/// Some basic tests to show it works.
/// Doesn't test any real logic except for one instantiation
pub fn happy_path<C>(chain: C)
where
    C: CwEnv + AltSigner,
    C::Sender: Addressable,
{
    let op1 = chain.alt_signer(1);
    let op2 = chain.alt_signer(2);
    let op3 = chain.alt_signer(3);
    let noop = chain.alt_signer(4);

    let operators = vec![
        (op1.addr().to_string(), 100u64),
        (op2.addr().to_string(), 200u64),
        (op3.addr().to_string(), 300u64),
    ];

    // put real message here
    let msg = InstantiateMsg { operators };
    let contract = setup(chain.clone(), msg);

    // now query the total power
    let total_power = contract.total_power_at_height(None).unwrap();
    assert_eq!(total_power.power, Uint128::from(600u64));
    assert_ne!(total_power.height, 0u64);

    // now query the total power
    let total_power = contract.total_power_at_height(Some(173)).unwrap();
    assert_eq!(total_power.power, Uint128::from(600u64));
    assert_eq!(total_power.height, 173u64);

    // query the power of an operator
    let total_power = contract
        .voting_power_at_height(op2.addr().into_string(), Some(287))
        .unwrap();
    assert_eq!(total_power.power, Uint128::from(200u64));
    assert_eq!(total_power.height, 287u64);

    // query the power of an operator with None height (should return the current height, just ensure it works)
    let total_power = contract
        .voting_power_at_height(op1.addr().into_string(), None)
        .unwrap();
    assert_eq!(total_power.power, Uint128::from(100u64));

    // query the power of a non-operator
    let total_power = contract
        .voting_power_at_height(noop.addr().into_string(), Some(287))
        .unwrap();
    assert_eq!(total_power.power, Uint128::zero());
    assert_eq!(total_power.height, 287u64);
}
