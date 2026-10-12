use crate::enums::DeploymentTarget;
use crate::enums::listed;
use crate::model::DeploymentSection;
use crate::report::ValidationResult;

pub(super) fn validate(out: &mut ValidationResult, section: DeploymentSection) {
    if section.target.is_none() {
        out.error(
            "deployment.target",
            format!(
                "Missing required field \"deployment.target\" — must be one of: {}",
                listed::<DeploymentTarget>()
            ),
        );
    }
}
