use crate::context::AppContext;
use anyhow::Result;
use layer_climb::prelude::*;
use oracle_verifier::msg::QueryMsg;
use oracle_verifier::state::Config;

pub struct OracleVerifierQuerier {
    pub ctx: AppContext,
    pub contract_addr: Address,
    pub querier: QueryClient,
}

impl OracleVerifierQuerier {
    pub async fn new(ctx: AppContext, contract_addr: Address) -> Result<Self> {
        Ok(Self {
            querier: ctx.query_client().await?,
            ctx,
            contract_addr,
        })
    }

    pub async fn config(&self) -> Result<Config> {
        self.querier
            .contract_smart(&self.contract_addr, &QueryMsg::Config {})
            .await
    }

    pub async fn operator_addr(&self) -> Result<Address> {
        let config = self.config().await?;
        self.ctx
            .chain_config()?
            .parse_address(&config.operator_contract.to_string())
    }
}
