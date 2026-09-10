//! How a spend budget is windowed -- quota vocabulary a provider limit is
//! stated in.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BudgetPeriod {
    /// Resets at the start of each calendar month.
    CalendarMonth,
    /// A trailing thirty days that never fully resets.
    RollingThirtyDays,
}

impl BudgetPeriod {
    pub fn as_str(self) -> &'static str {
        match self {
            BudgetPeriod::CalendarMonth => "calendar month",
            BudgetPeriod::RollingThirtyDays => "rolling thirty days",
        }
    }
}
