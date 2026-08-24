use chrono::{DateTime, Datelike, Timelike, Utc, Weekday};
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
    #[error("invalid weekday {0}; expected 1-7 or Monday-Sunday")]
    InvalidDay(String),
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
    #[serde(default)]
    pub site: Option<String>,
    pub model: String,
    pub currency: String,
    #[serde(default)]
    pub rates: Option<PriceRates>,
    #[serde(default)]
    pub fast: Option<PriceRates>,
    #[serde(default)]
    pub peak: Option<PriceRates>,
    #[serde(default)]
    pub off_peak: Option<PriceRates>,
    #[serde(default)]
    pub peak_windows: Option<Vec<PeakWindowConfig>>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum WeekdaySpec {
    Number(u8),
    Name(String),
}

impl WeekdaySpec {
    fn parse(&self) -> Result<Weekday, PricingError> {
        match self {
            Self::Number(n) => parse_day_number(*n),
            Self::Name(s) => parse_day_name(s),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PeakWindowConfig {
    Bounds {
        #[serde(default, alias = "days_of_week", alias = "day_of_week", alias = "days")]
        weekdays: Option<Vec<WeekdaySpec>>,
        start: String,
        end: String,
    },
    Legacy(String),
}

impl PeakWindowConfig {
    fn parse(&self) -> Result<(Vec<Weekday>, usize, usize), PricingError> {
        match self {
            Self::Bounds {
                weekdays,
                start,
                end,
            } => {
                let parsed_days = match weekdays {
                    Some(list) => {
                        let mut days_vec = Vec::with_capacity(list.len());
                        for spec in list {
                            days_vec.push(spec.parse()?);
                        }
                        if days_vec.is_empty() {
                            all_days()
                        } else {
                            days_vec
                        }
                    }
                    None => all_days(),
                };
                let (start_min, end_min) = parse_window_parts(start, end)?;
                Ok((parsed_days, start_min, end_min))
            }
            Self::Legacy(value) => {
                let (start_min, end_min) = parse_window(value)?;
                Ok((all_days(), start_min, end_min))
            }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeakSchedule {
    minutes: Box<[[bool; 1440]; 7]>,
}

impl Default for PeakSchedule {
    fn default() -> Self {
        Self {
            minutes: Box::new([[false; 1440]; 7]),
        }
    }
}

impl PeakSchedule {
    pub fn from_windows(windows: &[PeakWindowConfig]) -> Result<Self, PricingError> {
        let mut minutes = Box::new([[false; 1440]; 7]);
        for window in windows {
            let (days, start, end) = window.parse()?;
            for day in days {
                let day_idx = day.num_days_from_monday() as usize;
                if start < end {
                    for m in start..end {
                        if minutes[day_idx][m] {
                            return Err(PricingError::Overlap);
                        }
                        minutes[day_idx][m] = true;
                    }
                } else {
                    for m in start..1440 {
                        if minutes[day_idx][m] {
                            return Err(PricingError::Overlap);
                        }
                        minutes[day_idx][m] = true;
                    }
                    let next_day_idx = (day_idx + 1) % 7;
                    for m in 0..end {
                        if minutes[next_day_idx][m] {
                            return Err(PricingError::Overlap);
                        }
                        minutes[next_day_idx][m] = true;
                    }
                }
            }
        }
        Ok(Self { minutes })
    }

    pub fn is_peak(&self, weekday: Weekday, minute: usize) -> bool {
        let day_idx = weekday.num_days_from_monday() as usize;
        self.minutes[day_idx][minute]
    }

    pub fn period_at(&self, local: DateTime<Tz>) -> PricePeriod {
        let minute = (local.hour() * 60 + local.minute()) as usize;
        if self.is_peak(local.weekday(), minute) {
            PricePeriod::Peak
        } else {
            PricePeriod::OffPeak
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct FloatPriceRates {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

#[derive(Debug)]
struct CompiledRule {
    site: Option<String>,
    matcher: GlobMatcher,
    currency: String,
    peak: PriceRates,
    off_peak: PriceRates,
    fast: Option<PriceRates>,
    peak_f64: FloatPriceRates,
    off_peak_f64: FloatPriceRates,
    fast_f64: Option<FloatPriceRates>,
    schedule: PeakSchedule,
}

#[derive(Debug)]
pub struct PriceBook {
    timezone: Tz,
    default_schedule: PeakSchedule,
    rules: Vec<CompiledRule>,
}
pub struct ResolvedPrice<'a> {
    pub period: PricePeriod,
    pub currency: &'a str,
    rates: &'a PriceRates,
    float_rates: &'a FloatPriceRates,
}

impl ResolvedPrice<'_> {
    pub fn rate(&self, token_type: TokenType) -> Option<Decimal> {
        self.rates.rate(token_type)
    }

    pub fn rate_f64(&self, token_type: TokenType) -> Option<f64> {
        match token_type {
            TokenType::Input => self.float_rates.input,
            TokenType::Output => self.float_rates.output,
            TokenType::CacheRead => self.float_rates.cache_read,
            TokenType::CacheWrite => self.float_rates.cache_write,
        }
    }
}

impl PriceBook {
    pub fn from_config(config: &PricingConfig) -> Result<Self, PricingError> {
        let timezone = Tz::from_str(&config.timezone)
            .map_err(|_| PricingError::Timezone(config.timezone.clone()))?;
        let default_schedule = PeakSchedule::from_windows(&config.peak_windows)?;
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
                let schedule = match &rule.peak_windows {
                    Some(windows) => PeakSchedule::from_windows(windows)?,
                    None => default_schedule.clone(),
                };
                let site = rule
                    .site
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned);
                Ok(CompiledRule {
                    site,
                    matcher,
                    currency: rule.currency.clone(),
                    peak_f64: FloatPriceRates::from_rates(&peak),
                    off_peak_f64: FloatPriceRates::from_rates(&off_peak),
                    fast_f64: rule.fast.as_ref().map(FloatPriceRates::from_rates),
                    peak,
                    off_peak,
                    fast: rule.fast.clone(),
                    schedule,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            timezone,
            default_schedule,
            rules,
        })
    }

    pub fn period_at(&self, instant: DateTime<Utc>) -> PricePeriod {
        let local = instant.with_timezone(&self.timezone);
        self.default_schedule.period_at(local)
    }

    pub fn lookup(
        &self,
        site: &str,
        model: &str,
        instant: DateTime<Utc>,
    ) -> Option<ResolvedPrice<'_>> {
        let rule = self.matching_rule(site, model)?;
        let local = instant.with_timezone(&self.timezone);
        let period = rule.schedule.period_at(local);
        let (rates, float_rates) = match period {
            PricePeriod::Peak => (&rule.peak, &rule.peak_f64),
            PricePeriod::OffPeak => (&rule.off_peak, &rule.off_peak_f64),
        };
        Some(ResolvedPrice {
            period,
            currency: &rule.currency,
            rates,
            float_rates,
        })
    }

    pub fn lookup_fast(
        &self,
        site: &str,
        model: &str,
        instant: DateTime<Utc>,
    ) -> Option<ResolvedPrice<'_>> {
        let rule = self.matching_rule(site, model)?;
        let (rates, float_rates) = rule.fast.as_ref().zip(rule.fast_f64.as_ref())?;
        let local = instant.with_timezone(&self.timezone);
        Some(ResolvedPrice {
            period: rule.schedule.period_at(local),
            currency: &rule.currency,
            rates,
            float_rates,
        })
    }

    fn matching_rule(&self, site: &str, model: &str) -> Option<&CompiledRule> {
        self.rules
            .iter()
            .find(|rule| rule.site.as_deref() == Some(site) && rule.matcher.is_match(model))
            .or_else(|| {
                self.rules
                    .iter()
                    .find(|rule| rule.site.is_none() && rule.matcher.is_match(model))
            })
    }
}

impl FloatPriceRates {
    fn from_rates(rates: &PriceRates) -> Self {
        Self {
            input: decimal_to_f64(rates.input),
            output: decimal_to_f64(rates.output),
            cache_read: decimal_to_f64(rates.cache_read),
            cache_write: decimal_to_f64(rates.cache_write),
        }
    }
}

fn decimal_to_f64(value: Option<Decimal>) -> Option<f64> {
    value.and_then(|value| value.to_string().parse().ok())
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
fn all_days() -> Vec<Weekday> {
    vec![
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
    ]
}

fn parse_day_number(n: u8) -> Result<Weekday, PricingError> {
    match n {
        1 => Ok(Weekday::Mon),
        2 => Ok(Weekday::Tue),
        3 => Ok(Weekday::Wed),
        4 => Ok(Weekday::Thu),
        5 => Ok(Weekday::Fri),
        6 => Ok(Weekday::Sat),
        7 => Ok(Weekday::Sun),
        _ => Err(PricingError::InvalidDay(n.to_string())),
    }
}

fn parse_day_name(input: &str) -> Result<Weekday, PricingError> {
    match input.trim().to_lowercase().as_str() {
        "mon" | "monday" => Ok(Weekday::Mon),
        "tue" | "tues" | "tuesday" => Ok(Weekday::Tue),
        "wed" | "wednesday" => Ok(Weekday::Wed),
        "thu" | "thur" | "thurs" | "thursday" => Ok(Weekday::Thu),
        "fri" | "friday" => Ok(Weekday::Fri),
        "sat" | "saturday" => Ok(Weekday::Sat),
        "sun" | "sunday" => Ok(Weekday::Sun),
        _ => {
            if let Ok(n) = input.trim().parse::<u8>() {
                parse_day_number(n)
            } else {
                Err(PricingError::InvalidDay(input.to_owned()))
            }
        }
    }
}
