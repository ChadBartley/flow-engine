use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub prompt: f64,     // Cost per token
    pub completion: f64, // Cost per token
}

#[derive(Debug, Clone)]
pub struct PricingCalculator {
    pricing: HashMap<String, ModelPricing>,
}

impl PricingCalculator {
    pub fn new() -> Self {
        Self {
            pricing: HashMap::new(),
        }
    }

    pub fn add_model(&mut self, model: String, pricing: ModelPricing) {
        self.pricing.insert(model, pricing);
    }

    pub fn get_pricing(&self, model: &str) -> Option<&ModelPricing> {
        self.pricing.get(model)
    }

    pub fn calculate_cost(
        &self,
        model: &str,
        prompt_tokens: u64,
        completion_tokens: u64,
    ) -> Option<f64> {
        self.pricing
            .get(model)
            .map(|p| (prompt_tokens as f64 * p.prompt) + (completion_tokens as f64 * p.completion))
    }

    pub fn estimate_prompt_tokens(text: &str) -> u64 {
        // Rough estimate: ~4 characters per token
        (text.len() as f64 / 4.0).ceil() as u64
    }
}

impl Default for PricingCalculator {
    fn default() -> Self {
        Self::new()
    }
}
