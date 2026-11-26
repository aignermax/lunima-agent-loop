use crate::config::Budgets;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectBudgets {
    pub product_owner_max_turns: Option<u32>,
    pub product_owner_timeout_minutes: Option<u64>,
    pub discovery_max_turns: Option<u32>,
    pub discovery_timeout_minutes: Option<u64>,
    pub timeout_minutes_regular: Option<u64>,
    pub timeout_minutes_complex: Option<u64>,
    pub max_turns_regular: Option<u32>,
    pub max_turns_complex: Option<u32>,
    pub max_review_rounds_regular: Option<u32>,
    pub max_review_rounds_complex: Option<u32>,
    pub max_qa_fix_rounds: Option<u32>,
    pub reviewer_max_turns: Option<u32>,
    pub pr_feedback_max_rounds: Option<u32>,
    pub pr_feedback_max_turns: Option<u32>,
    pub max_sessions: Option<u32>,
    pub inactivity_minutes: Option<u64>,
    pub max_cost_usd_regular: Option<f64>,
    pub max_cost_usd_complex: Option<f64>,
    pub max_tokens_regular: Option<u64>,
    pub max_tokens_complex: Option<u64>,
    pub max_attempts: Option<u32>,
    pub test_gate: Option<bool>,
}

impl ProjectBudgets {
    pub fn apply(&self, base: &Budgets) -> Budgets {
        Budgets {
            timeout_minutes_regular: self
                .timeout_minutes_regular
                .unwrap_or(base.timeout_minutes_regular),
            timeout_minutes_complex: self
                .timeout_minutes_complex
                .unwrap_or(base.timeout_minutes_complex),
            max_turns_regular: self.max_turns_regular.unwrap_or(base.max_turns_regular),
            max_turns_complex: self.max_turns_complex.unwrap_or(base.max_turns_complex),
            max_review_rounds_regular: self
                .max_review_rounds_regular
                .unwrap_or(base.max_review_rounds_regular),
            max_review_rounds_complex: self
                .max_review_rounds_complex
                .unwrap_or(base.max_review_rounds_complex),
            max_qa_fix_rounds: self.max_qa_fix_rounds.unwrap_or(base.max_qa_fix_rounds),
            reviewer_max_turns: self.reviewer_max_turns.unwrap_or(base.reviewer_max_turns),
            pr_feedback_max_rounds: self
                .pr_feedback_max_rounds
                .unwrap_or(base.pr_feedback_max_rounds),
            pr_feedback_max_turns: self
                .pr_feedback_max_turns
                .unwrap_or(base.pr_feedback_max_turns),
            max_sessions: self.max_sessions.unwrap_or(base.max_sessions),
            inactivity_minutes: self.inactivity_minutes.unwrap_or(base.inactivity_minutes),
            max_cost_usd_regular: self.max_cost_usd_regular.unwrap_or(base.max_cost_usd_regular),
            max_cost_usd_complex: self.max_cost_usd_complex.unwrap_or(base.max_cost_usd_complex),
            max_tokens_regular: self.max_tokens_regular.unwrap_or(base.max_tokens_regular),
            max_tokens_complex: self.max_tokens_complex.unwrap_or(base.max_tokens_complex),
            max_attempts: self.max_attempts.unwrap_or(base.max_attempts),
            test_gate: self.test_gate.unwrap_or(base.test_gate),
        }
    }
}
