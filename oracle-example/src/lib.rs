#[allow(warnings)]
mod bindings;
use bindings::{Guest, Output, TaskQueueInput};

use layer_wasi::{block_on, Reactor, Request, WasiPollable};

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

const PRICE_HISTORY_FILE_PATH: &str = "price_history.json";

struct Component;

impl Guest for Component {
    fn run_task(_input: TaskQueueInput) -> Output {
        block_on(get_avg_btc)
    }
}

async fn get_avg_btc(reactor: Reactor) -> Result<Vec<u8>, String> {
    let api_key = std::env::var("API_KEY").or(Err("missing env var `API_KEY`".to_string()))?;
    let price = get_btc_usd_price(&reactor, &api_key)
        .await
        .map_err(|err| err.to_string())?
        .ok_or("invalid response from coin gecko API")?;

    // read previous price history
    let mut history = match std::fs::read(PRICE_HISTORY_FILE_PATH) {
        Ok(bytes) => {
            serde_json::from_slice::<PriceHistory>(&bytes).map_err(|err| err.to_string())?
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(err) => return Err(err.to_string()),
    };

    // get current time in secs
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("failed to get current time")
        .as_secs();

    // add latest price to front of the list and truncate to max of 1000
    history.btcusd_prices.push_front((now, price));
    history.btcusd_prices.truncate(1000);

    // write price history
    std::fs::write(
        PRICE_HISTORY_FILE_PATH,
        serde_json::to_vec(&history).map_err(|err| err.to_string())?,
    )
    .map_err(|err| err.to_string())?;

    // calculate average prices
    let avg_last_minute = history.average(now - 60);
    let avg_last_hour = history.average(now - 3600);

    // serialize JSON response
    serde_json::to_vec(&Response {
        btcusd: Price {
            price,
            avg_last_minute,
            avg_last_hour,
        },
    })
    .map_err(|err| err.to_string())
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Response {
    pub btcusd: Price,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Price {
    pub price: f32,
    pub avg_last_minute: AveragePrice,
    pub avg_last_hour: AveragePrice,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
struct AveragePrice {
    pub price: f32,
    pub count: usize,
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
struct PriceHistory {
    pub btcusd_prices: VecDeque<(u64, f32)>,
}

impl PriceHistory {
    fn average(&self, since_time_secs: u64) -> AveragePrice {
        let mut sum = 0f64;
        let mut count = 0;
        for (t, p) in self.btcusd_prices.iter() {
            if t >= &since_time_secs {
                sum += *p as f64;
                count += 1;
            } else {
                break;
            }
        }
        AveragePrice {
            price: (sum / (count as f64)) as f32,
            count,
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct CoinInfo {
    pub value: f32,
}

#[derive(Deserialize, Debug)]
pub struct CoinGeckoResponse {
    pub rates: HashMap<String, CoinInfo>,
}

impl CoinGeckoResponse {
    fn btc_usd(&self) -> Option<f32> {
        self.rates.get("usd").map(|info| info.value)
    }
}

pub async fn get_btc_usd_price(reactor: &Reactor, api_key: &str) -> Result<Option<f32>, String> {
    let mut req = Request::get("https://api.coingecko.com/api/v3/exchange_rates")?;
    req.headers = vec![("x-cg-pro-api-key".to_string(), api_key.to_owned())];
    let res = reactor.send(req).await?;

    match res.status {
        200 => res.json::<CoinGeckoResponse>().map(|rates| rates.btc_usd()),
        429 => Err("rate limited, price unavailable".to_string()),
        status => Err(format!("unexpected status code: {status}")),
    }
}

bindings::export!(Component with_types_in bindings);
