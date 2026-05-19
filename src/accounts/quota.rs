use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Default)]
pub struct QuotaSnapshot {
    pub five_hour_util: Option<f32>,
    pub seven_day_util: Option<f32>,
    pub five_hour_reset_at: Option<DateTime<Utc>>,
    pub seven_day_reset_at: Option<DateTime<Utc>>,
    pub fallback_percentage: Option<f32>,
    pub overage_status: Option<String>,
}
