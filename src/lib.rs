#[macro_use]
extern crate seed;
use chrono::serde::ts_milliseconds;
use chrono::{DateTime, Utc};
use futures::Future;
use num::bigint::BigUint;
use num::integer::Integer;
use num::traits::ToPrimitive;
use num_format::{Locale, ToFormattedString};
use seed::prelude::*;
use seed::{Method, Request};
use serde::Deserialize;
use std::collections::HashMap;

mod ad_unit;
use ad_unit::*;
mod targeting_tag;
use targeting_tag::*;
mod validator;
use validator::*;
mod event_submission;
use event_submission::*;
mod channel;
use channel::*;

mod bignum;
use bignum::*;

const MARKET_URL: &str = "https://market.adex.network/campaigns?all";
const DAI_ADDR: &str = "0x89d24A6b4CcB1B6fAA2625fE562bDD9a23260359";
const UPDATE_MS: i32 = 30000;

// Data structs specific to the market
#[derive(Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MarketStatusType {
    Initializing,
    Ready,
    Active,
    Offline,
    Disconnected,
    Unhealthy,
    Withdraw,
    Expired,
    Exhausted,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct Spec {
    min_per_impression: BigNum,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct MarketStatus {
    #[serde(rename = "name")]
    pub status_type: MarketStatusType,
    pub usd_estimate: f32,
    #[serde(rename = "lastApprovedBalances")]
    pub balances: HashMap<String, BigNum>,
    #[serde(with = "ts_milliseconds")]
    pub last_checked: DateTime<Utc>,
}
impl MarketStatus {
    fn balances_sum(&self) -> BigUint {
        self.balances.iter().map(|(_, v)| &v.0).sum()
    }
}
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct MarketChannel {
    pub id: String,
    pub deposit_asset: String,
    pub deposit_amount: BigNum,
    pub status: MarketStatus,
    pub spec: ChannelSpec,
}

// Model
enum Loadable<T> {
    Loading,
    Ready(T),
}
enum ChannelSort {
    Deposit,
    Status,
}
struct Model {
    pub channels: Loadable<Vec<MarketChannel>>,
    pub sort: ChannelSort,
}
impl Default for Model {
    fn default() -> Self {
        Model {
            channels: Loadable::Loading,
            sort: ChannelSort::Deposit,
        }
    }
}

// Update
#[derive(Clone)]
enum Msg {
    LoadCampaigns,
    ChannelsLoaded(Vec<MarketChannel>),
    OnFetchErr(JsValue),
    SortSelected(String),
}

fn update(msg: Msg, model: &mut Model, orders: &mut Orders<Msg>) {
    match msg {
        Msg::LoadCampaigns => {
            let order = Request::new(MARKET_URL)
                .method(Method::Get)
                .fetch_json()
                .map(Msg::ChannelsLoaded)
                .map_err(Msg::OnFetchErr);
            orders.skip().perform_cmd(order);
        }
        Msg::ChannelsLoaded(channels) => model.channels = Loadable::Ready(channels),
        // @TODO handle this
        Msg::OnFetchErr(_) => (),
        Msg::SortSelected(sort_name) => match &sort_name as &str {
            "deposit" => model.sort = ChannelSort::Deposit,
            "status" => model.sort = ChannelSort::Status,
            _ => (),
        },
    }
}

// View
fn view(model: &Model) -> El<Msg> {
    let channels = match &model.channels {
        Loadable::Loading => return h2!["Loading..."],
        Loadable::Ready(c) => c,
    };

    let total_impressions: u64 = channels
        .iter()
        .map(|x| {
            (&x.status.balances_sum() / &x.spec.min_per_impression.0)
                .to_u64()
                .unwrap_or(0)
        })
        .sum();

    // @TODO we can make a special type for DAI channels and that way shield ourselves of
    // rendering wrongly
    let mut channels_dai: Vec<MarketChannel> = channels
        .iter()
        .filter(|MarketChannel { deposit_asset, .. }| deposit_asset == DAI_ADDR)
        .cloned()
        .collect();

    let total_paid: BigUint = channels_dai.iter().map(|x| x.status.balances_sum()).sum();

    match model.sort {
        ChannelSort::Deposit => {
            channels_dai.sort_by(|x, y| y.deposit_amount.0.cmp(&x.deposit_amount.0));
        }
        ChannelSort::Status => channels_dai.sort_by_key(|x| x.status.status_type.clone()),
    }

    let total_deposit: BigUint = channels_dai
        .iter()
        .map(|MarketChannel { deposit_amount, .. }| &deposit_amount.0)
        .sum();

    div![
        h2![format!(
            "Total campaign deposits: {}",
            dai_readable(&total_deposit)
        )],
        h2![format!("Total paid: {}", dai_readable(&total_paid))],
        h2![
            //attrs!{ At::Class => "impressions-rainbow" },
            format!(
                "Total impressions: {}",
                total_impressions.to_formatted_string(&Locale::en)
            )
        ],
        select![
            attrs! {At::Value => "deposit"},
            option![attrs! {At::Value => "deposit"}, "Sort by deposit"],
            option![attrs! {At::Value => "status"}, "Sort by status"],
            input_ev(Ev::Input, Msg::SortSelected)
        ],
        table![view_channel_table(&channels_dai)]
    ]
}

fn view_channel_table(channels: &[MarketChannel]) -> Vec<El<Msg>> {
    let rows = channels.iter().map(view_channel);

    let header = tr![
        td!["URL"],
        td!["USD estimate"],
        td!["Deposit"],
        td!["Paid"],
        td!["Paid - %"],
        td!["Status"],
        td!["Last updated"],
    ];

    std::iter::once(header)
        .chain(rows)
        .collect::<Vec<El<Msg>>>()
}

fn view_channel(channel: &MarketChannel) -> El<Msg> {
    let deposit_amount = &channel.deposit_amount.0;
    let paid_total = channel.status.balances_sum();
    let url = format!(
        "{}/channel/{}/status",
        channel.spec.validators.get(0).map_or("", |v| &v.url),
        channel.id
    );
    let id_prefix = channel.id.chars().take(6).collect::<String>();
    tr![
        td![a![
            attrs! {At::Href => url; At::Target => "_blank"},
            id_prefix
        ]],
        td![format!("${:.2}", &channel.status.usd_estimate)],
        td![dai_readable(&deposit_amount)],
        td![dai_readable(&paid_total)],
        td![{
            let base = 100000u32;
            let paid_units = (paid_total * base).div_floor(deposit_amount);
            let paid_hundreds = paid_units.to_f64().unwrap_or(base as f64) / (base as f64 / 100.0);
            format!("{:.3}%", paid_hundreds)
        }],
        td![format!("{:?}", &channel.status.status_type)],
        td![{
            let last_checked = &channel.status.last_checked;
            /*
            let time_diff = last_checked.signed_duration_since(Utc::now());
            match time_diff.num_seconds() {
                x @ 0..=59 => format!("{} seconds ago", x),
                x @ 60..=3600 => format!("{} minutes ago", x/60),
                _ => format!("{}", last_checked.format("%Y-%m-%d"))
            }*/
            format!("{}", last_checked.format("%Y-%m-%d"))
        }]
    ]
}
fn dai_readable(bal: &BigUint) -> String {
    // 10 ** 16
    match bal.div_floor(&10_000_000_000_000_000u64.into()).to_f64() {
        Some(hundreds) => format!("{:.2} DAI", hundreds / 100.0),
        None => ">max".to_owned(),
    }
}

#[wasm_bindgen]
pub fn render() {
    let state = seed::App::build(Model::default(), update, view)
        .finish()
        .run();

    state.update(Msg::LoadCampaigns);
    seed::set_interval(
        Box::new(move || state.update(Msg::LoadCampaigns)),
        UPDATE_MS,
    );
}
