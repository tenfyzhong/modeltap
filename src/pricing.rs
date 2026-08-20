use chrono::{DateTime, Timelike, Utc};
use chrono_tz::Tz;
use globset::{Glob, GlobMatcher};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PricingError {
    #[error("invalid timezone {0}")]
    Timezone(String),
    #[error("invalid peak window {0}; expected HH:MM-HH:MM")]
    Window(String),
    #[error("peak windows overlap")]
    Overlap,
    #[error("invalid model glob {0}")]
    Glob(String),
    #[error("price rule for {0} has no currency")]
    Currency(String),
    #[error("price rule for {0} must define either rates or both peak and off_peak rates")]
    Rates(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct PricingConfig {
    pub timezone: String,
    #[serde(default)]
    pub peak_windows: Vec<PeakWindowConfig>,
    #[serde(default)]
    pub rules: Vec<PriceRuleConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PriceRuleConfig {
    pub site: String,
    pub model: String,
    pub currency: String,
    #[serde(default)]
    pub rates: Option<PriceRates>,
    #[serde(default)]
    pub peak: Option<PriceRates>,
    #[serde(default)]
    pub off_peak: Option<PriceRates>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PeakWindowConfig {
    Bounds { start: String, end: String },
    Legacy(String),
}

impl PeakWindowConfig {
    fn parse(&self) -> Result<(usize, usize), PricingError> {
        match self {
            Self::Bounds { start, end } => parse_window_parts(start, end),
            Self::Legacy(value) => parse_window(value),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PriceRates {
    pub input: Option<Decimal>,
    pub output: Option<Decimal>,
    pub cache_read: Option<Decimal>,
    pub cache_write: Option<Decimal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PricePeriod {
    Peak,
    OffPeak,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenType {
    Input,
    Output,
    CacheRead,
    CacheWrite,
}

impl PriceRates {
    pub fn rate(&self, token_type: TokenType) -> Option<Decimal> {
        match token_type {
            TokenType::Input => self.input,
            TokenType::Output => self.output,
            TokenType::CacheRead => self.cache_read,
            TokenType::CacheWrite => self.cache_write,
        }
    }
}

#[derive(Debug)]
struct CompiledRule {
    site: String,
    matcher: GlobMatcher,
    currency: String,
    peak: PriceRates,
    off_peak: PriceRates,
}

#[derive(Debug)]
pub struct PriceBook {
    timezone: Tz,
    peak_minutes: Vec<bool>,
    rules: Vec<CompiledRule>,
}

pub struct ResolvedPrice<'a> {
    pub period: PricePeriod,
    pub currency: &'a str,
    rates: &'a PriceRates,
}

impl ResolvedPrice<'_> {
    pub fn rate(&self, token_type: TokenType) -> Option<Decimal> {
        self.rates.rate(token_type)
    }
}

impl PriceBook {
    pub fn from_config(config: &PricingConfig) -> Result<Self, PricingError> {
        let timezone = Tz::from_str(&config.timezone)
            .map_err(|_| PricingError::Timezone(config.timezone.clone()))?;
        let mut peak_minutes = vec![false; 24 * 60];
        for window in &config.peak_windows {
            let (start, end) = window.parse()?;
            let mut minute = start;
            loop {
                if peak_minutes[minute] {
                    return Err(PricingError::Overlap);
                }
                peak_minutes[minute] = true;
                minute = (minute + 1) % peak_minutes.len();
                if minute == end {
                    break;
                }
            }
        }
        let rules = config
            .rules
            .iter()
            .map(|rule| {
                if rule.currency.trim().is_empty() {
                    return Err(PricingError::Currency(rule.model.clone()));
                }
                let matcher = Glob::new(&rule.model)
                    .map_err(|_| PricingError::Glob(rule.model.clone()))?
                    .compile_matcher();
                let (peak, off_peak) = match (&rule.rates, &rule.peak, &rule.off_peak) {
                    (Some(rates), None, None) => (rates.clone(), rates.clone()),
                    (None, Some(peak), Some(off_peak)) => (peak.clone(), off_peak.clone()),
                    _ => return Err(PricingError::Rates(rule.model.clone())),
                };
                Ok(CompiledRule {
                    site: rule.site.clone(),
                    matcher,
                    currency: rule.currency.clone(),
                    peak,
                    off_peak,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            timezone,
            peak_minutes,
            rules,
        })
    }

    pub fn period_at(&self, instant: DateTime<Utc>) -> PricePeriod {
        let local = instant.with_timezone(&self.timezone);
        let minute = (local.hour() * 60 + local.minute()) as usize;
        if self.peak_minutes[minute] {
            PricePeriod::Peak
        } else {
            PricePeriod::OffPeak
        }
    }

    pub fn lookup(
        &self,
        site: &str,
        model: &str,
        instant: DateTime<Utc>,
    ) -> Option<ResolvedPrice<'_>> {
        let rule = self
            .rules
            .iter()
            .find(|rule| rule.site == site && rule.matcher.is_match(model))?;
        let period = self.period_at(instant);
        let rates = match period {
            PricePeriod::Peak => &rule.peak,
            PricePeriod::OffPeak => &rule.off_peak,
        };
        Some(ResolvedPrice {
            period,
            currency: &rule.currency,
            rates,
        })
    }
}

fn parse_window(input: &str) -> Result<(usize, usize), PricingError> {
    let (start, end) = input
        .split_once('-')
        .ok_or_else(|| PricingError::Window(input.to_owned()))?;
    parse_window_parts(start, end)
}

fn parse_window_parts(start: &str, end: &str) -> Result<(usize, usize), PricingError> {
    let input = format!("{start}-{end}");
    let start = parse_time(start).ok_or_else(|| PricingError::Window(input.clone()))?;
    let end = parse_time(end).ok_or_else(|| PricingError::Window(input.clone()))?;
    if start == end {
        return Err(PricingError::Window(input.to_owned()));
    }
    Ok((start, end))
}

fn parse_time(input: &str) -> Option<usize> {
    let (hour, minute) = input.split_once(':')?;
    let hour = hour.parse::<usize>().ok()?;
    let minute = minute.parse::<usize>().ok()?;
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
}
