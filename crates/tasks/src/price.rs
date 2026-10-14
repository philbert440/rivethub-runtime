use std::collections::BTreeMap;

use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenRate {
    pub(crate) input_per_mtok: f64,
    pub(crate) output_per_mtok: f64,
}

impl TokenRate {
    pub fn new(input_per_mtok: f64, output_per_mtok: f64) -> Result<Self, Error> {
        if !legal(input_per_mtok) || !legal(output_per_mtok) {
            return Err(Error::Pricing);
        }
        Ok(Self {
            input_per_mtok,
            output_per_mtok,
        })
    }
}

pub(crate) fn table(
    rates: BTreeMap<String, TokenRate>,
) -> Result<BTreeMap<String, TokenRate>, Error> {
    for rate in rates.values() {
        if !legal(rate.input_per_mtok) || !legal(rate.output_per_mtok) {
            return Err(Error::Pricing);
        }
    }
    Ok(rates)
}

pub(crate) fn cost(
    rates: &BTreeMap<String, TokenRate>,
    model: Option<&str>,
    input_tokens: u64,
    output_tokens: u64,
) -> Result<Option<f64>, Error> {
    let Some(model) = model else {
        return Ok(None);
    };
    let Some(rate) = rates.get(model) else {
        return Err(Error::Pricing);
    };
    let input = tokens_f64(input_tokens)?;
    let output = tokens_f64(output_tokens)?;
    let total =
        input * rate.input_per_mtok / 1_000_000.0 + output * rate.output_per_mtok / 1_000_000.0;
    if !total.is_finite() {
        return Err(Error::Pricing);
    }
    Ok(Some(total))
}

fn legal(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

fn tokens_f64(value: u64) -> Result<f64, Error> {
    const EXACT: u64 = 1_u64 << 53;
    if value > EXACT {
        return Err(Error::Pricing);
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "token counts above 2^53 are rejected before this conversion"
    )]
    Ok(value as f64)
}

#[cfg(test)]
mod tests {
    use super::TokenRate;
    use super::cost;
    use super::table;
    use std::collections::BTreeMap;

    #[test]
    fn rates_reject_values_that_are_not_finite_and_non_negative() {
        assert!(TokenRate::new(f64::NAN, 1.0).is_err());
        assert!(TokenRate::new(f64::INFINITY, 0.0).is_err());
        assert!(TokenRate::new(-1.0, 1.0).is_err());
        let mut rates = BTreeMap::new();
        rates.insert(
            "demo".to_string(),
            TokenRate {
                input_per_mtok: f64::NAN,
                output_per_mtok: 0.0,
            },
        );
        assert!(table(rates).is_err());
    }

    #[test]
    fn one_million_input_tokens_at_three_dollars_costs_three() {
        let mut rates = BTreeMap::new();
        rates.insert("demo".to_string(), TokenRate::new(3.0, 6.0).expect("rate"));
        let priced = cost(&rates, Some("demo"), 1_000_000, 0).expect("cost");
        assert_eq!(priced, Some(3.0));
        assert!(cost(&rates, Some("other"), 1, 1).is_err());
        assert_eq!(cost(&rates, None, 5, 5).expect("unpriced"), None);
        let over = (1_u64 << 53) + 1;
        assert!(cost(&rates, Some("demo"), over, 0).is_err());
    }
}
