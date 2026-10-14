use agent_client_protocol::schema::v1 as acp;
use kernel::harness::SessionOption;
use parking_lot::Mutex;

use crate::error::AcpError;

#[derive(Debug)]
pub(crate) struct ConfigSelection {
    pub(crate) config_id: acp::SessionConfigId,
    pub(crate) value: acp::SessionConfigOptionValue,
}

#[derive(Debug, Default)]
pub(crate) struct SessionOptions {
    options: Mutex<Vec<acp::SessionConfigOption>>,
}

impl SessionOptions {
    pub(crate) fn replace(&self, options: Vec<acp::SessionConfigOption>) {
        *self.options.lock() = options;
    }

    pub(crate) fn select(&self, option: &SessionOption) -> Result<ConfigSelection, AcpError> {
        let options = self.options.lock();
        select(&options, option)
    }
}

pub(crate) fn stored_options(
    options: Option<Vec<acp::SessionConfigOption>>,
) -> Vec<acp::SessionConfigOption> {
    let mut stored = Vec::new();
    if let Some(options) = options {
        stored.extend(options);
    }
    stored
}

fn select(
    options: &[acp::SessionConfigOption],
    option: &SessionOption,
) -> Result<ConfigSelection, AcpError> {
    let (category, value, label) = match option {
        SessionOption::Model(model) => (
            acp::SessionConfigOptionCategory::Model,
            model.clone(),
            "model",
        ),
        SessionOption::Effort(level) => (
            acp::SessionConfigOptionCategory::ThoughtLevel,
            level.to_string(),
            "thought_level",
        ),
    };
    let found = options
        .iter()
        .find(|item| item.category.as_ref() == Some(&category));
    let Some(found) = found else {
        return Err(AcpError::ConfigOptionUnavailable { category: label });
    };
    let offered = select_ids(found, label)?;
    if !offered.iter().any(|id| id.0.as_ref() == value) {
        return Err(AcpError::ConfigValueRejected {
            category: label,
            value,
        });
    }
    Ok(ConfigSelection {
        config_id: found.id.clone(),
        value: acp::SessionConfigOptionValue::value_id(value),
    })
}

fn select_ids(
    option: &acp::SessionConfigOption,
    category: &'static str,
) -> Result<Vec<acp::SessionConfigValueId>, AcpError> {
    match &option.kind {
        acp::SessionConfigKind::Select(select) => Ok(offered_ids(&select.options)),
        acp::SessionConfigKind::Boolean(_) | _ => {
            Err(AcpError::ConfigOptionUnavailable { category })
        }
    }
}

fn offered_ids(options: &acp::SessionConfigSelectOptions) -> Vec<acp::SessionConfigValueId> {
    match options {
        acp::SessionConfigSelectOptions::Ungrouped(items) => {
            items.iter().map(|item| item.value.clone()).collect()
        }
        acp::SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| group.options.iter().map(|item| item.value.clone()))
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::select;
    use agent_client_protocol::schema::v1 as acp;
    use kernel::harness::SessionOption;
    use wire::stream::ThinkingLevel;

    use crate::error::AcpError;

    fn model_option(current: &str) -> acp::SessionConfigOption {
        acp::SessionConfigOption::select(
            "model",
            "Model",
            current.to_owned(),
            vec![
                acp::SessionConfigSelectOption::new("grok-5", "grok-5"),
                acp::SessionConfigSelectOption::new("grok-4.7", "grok-4.7"),
            ],
        )
        .category(acp::SessionConfigOptionCategory::Model)
    }

    fn effort_option() -> acp::SessionConfigOption {
        acp::SessionConfigOption::select(
            "reasoning_effort",
            "Reasoning effort",
            "high",
            vec![acp::SessionConfigSelectGroup::new(
                "levels",
                "Levels",
                vec![
                    acp::SessionConfigSelectOption::new("low", "low"),
                    acp::SessionConfigSelectOption::new("high", "high"),
                ],
            )],
        )
        .category(acp::SessionConfigOptionCategory::ThoughtLevel)
    }

    #[test]
    fn model_and_grouped_effort_resolve_to_advertised_ids() {
        let options = vec![model_option("grok-4.7"), effort_option()];
        let model = select(&options, &SessionOption::Model("grok-5".to_owned())).expect("model");
        assert_eq!(model.config_id.0.as_ref(), "model");
        assert_eq!(
            model.value,
            acp::SessionConfigOptionValue::value_id("grok-5")
        );
        let effort = select(&options, &SessionOption::Effort(ThinkingLevel::Low)).expect("effort");
        assert_eq!(effort.config_id.0.as_ref(), "reasoning_effort");
        assert_eq!(effort.value, acp::SessionConfigOptionValue::value_id("low"));
    }

    #[test]
    fn a_missing_category_and_a_rejected_value_fail_closed() {
        let options = vec![model_option("grok-4.7")];
        let missing = select(&options, &SessionOption::Effort(ThinkingLevel::High))
            .expect_err("no thought level");
        assert!(matches!(
            missing,
            AcpError::ConfigOptionUnavailable {
                category: "thought_level"
            }
        ));
        let rejected = select(&options, &SessionOption::Model("missing".to_owned()))
            .expect_err("value not offered");
        assert!(matches!(
            rejected,
            AcpError::ConfigValueRejected {
                category: "model",
                ..
            }
        ));
    }

    #[test]
    fn a_boolean_option_is_not_a_model_selector() {
        let options = vec![
            acp::SessionConfigOption::boolean("model", "Model", true)
                .category(acp::SessionConfigOptionCategory::Model),
        ];
        let error = select(&options, &SessionOption::Model("grok-5".to_owned()))
            .expect_err("boolean is not selectable");
        assert!(matches!(
            error,
            AcpError::ConfigOptionUnavailable { category: "model" }
        ));
    }
}
