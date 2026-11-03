//! NBP exchange rates, proxied and cached. The client asks for a currency and a quarter, never for
//! a purchase date, and NBP sees this server instead of the user's address.

use crate::{State, auth::AuthUser, error::{Error, Result}};
use axum::{Json, extract::Path};
use mm_core::api::{Rate, RatesResp};
use mm_core::date::Date;
use mm_core::money::parse_rate;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A past quarter never changes. The current one gains a rate each business day.
const FRESH: Duration = Duration::from_secs(6 * 3600);

/// Currency, year and quarter.
type Quarter = (String, i32, u32);

#[derive(Default)]
pub struct Cache(Mutex<HashMap<Quarter, (Instant, Vec<Rate>)>>);

#[derive(Deserialize)]
struct NbpTable {
    rates: Vec<NbpRate>,
}

#[derive(Deserialize)]
struct NbpRate {
    #[serde(rename = "effectiveDate")]
    date: String,
    mid: serde_json::Number,
}

fn quarter_range(year: i32, quarter: u32) -> Option<(Date, Date)> {
    if !(1..=4).contains(&quarter) || !(2002..=2100).contains(&year) {
        return None;
    }
    let first = Date::new(year, quarter * 3 - 2, 1)?;
    let last = mm_core::date::Month { year, month: quarter * 3 }.last();
    Some((first, last))
}

pub async fn get(s: State, AuthUser(_): AuthUser, Path((code, year, quarter)): Path<(String, i32, u32)>) -> Result<Json<RatesResp>> {
    let code = code.to_uppercase();
    if code.len() != 3 || !code.chars().all(|c| c.is_ascii_uppercase()) || code == "PLN" {
        return Err(Error::BadRequest("currency"));
    }
    let (first, last) = quarter_range(year, quarter).ok_or(Error::BadRequest("quarter"))?;
    let key = (code.clone(), year, quarter);
    if let Some((at, rates)) = s.rates.0.lock().map_err(|_| Error::Internal)?.get(&key)
        && at.elapsed() < FRESH
    {
        return Ok(Json(RatesResp { currency: code, rates: rates.clone() }));
    }
    let url = format!("{}/api/exchangerates/rates/a/{}/{}/{}/?format=json", s.nbp_url, code.to_lowercase(), first.iso(), last.iso());
    let resp = s.http.get(&url).timeout(Duration::from_secs(15)).send().await.map_err(|_| Error::Internal)?;
    let rates = match resp.status().as_u16() {
        // NBP answers 404 when the range has no business day yet, which simply means no rates.
        404 => vec![],
        200 => {
            let t: NbpTable = resp.json().await.map_err(|_| Error::Internal)?;
            t.rates.into_iter()
                .filter_map(|r| Some(Rate { rate_e4: parse_rate(&r.mid.to_string())?, date: r.date }))
                .collect()
        }
        _ => return Err(Error::Internal),
    };
    s.rates.0.lock().map_err(|_| Error::Internal)?.insert(key, (Instant::now(), rates.clone()));
    Ok(Json(RatesResp { currency: code, rates }))
}
