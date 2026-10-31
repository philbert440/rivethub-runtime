use kernel::harness::{
    EffortOption, HarnessCapabilities, HarnessControlCapabilities, HarnessModelOption,
    HarnessModelsSource, HarnessObservationCapabilities, HarnessOffering,
};

pub(crate) fn harness_offering_body(value: HarnessOffering) -> wire::harness::HarnessOffering {
    wire::harness::HarnessOffering {
        harness_id: value.harness_id,
        chat: value.chat,
        capabilities: harness_capabilities_body(value.capabilities),
    }
}

fn harness_capabilities_body(value: HarnessCapabilities) -> wire::harness::HarnessCapabilities {
    wire::harness::HarnessCapabilities {
        control: harness_control_capabilities_body(value.control),
        observation: harness_observation_capabilities_body(value.observation),
        turn_options: value.turn_options,
        launch_model: value.launch_model,
        image_attachments: value.image_attachments,
        models: value
            .models
            .map(|v| v.into_iter().map(harness_model_option_body).collect()),
        models_source: value.models_source.map(harness_models_source_body),
        efforts: value
            .efforts
            .map(|v| v.into_iter().map(effort_option_body).collect()),
        model_flag: value.model_flag,
        effort_flag: value.effort_flag,
    }
}

fn harness_control_capabilities_body(
    value: HarnessControlCapabilities,
) -> wire::harness::HarnessControlCapabilities {
    wire::harness::HarnessControlCapabilities {
        interrupt: value.interrupt,
        resume: value.resume,
        approvals: value.approvals,
        steering: value.steering,
    }
}

fn harness_observation_capabilities_body(
    value: HarnessObservationCapabilities,
) -> wire::harness::HarnessObservationCapabilities {
    wire::harness::HarnessObservationCapabilities {
        live_stream: value.live_stream,
        list_sessions: value.list_sessions,
        drive: value.drive,
    }
}

fn harness_model_option_body(value: HarnessModelOption) -> wire::harness::HarnessModelOption {
    wire::harness::HarnessModelOption {
        input_modalities: value.input_modalities,
        id: value.id,
        label: value.label,
        is_default: value.is_default,
        efforts: value
            .efforts
            .map(|v| v.into_iter().map(effort_option_body).collect()),
    }
}

const fn harness_models_source_body(
    value: HarnessModelsSource,
) -> wire::harness::HarnessModelsSource {
    match value {
        HarnessModelsSource::Discovered => wire::harness::HarnessModelsSource::Discovered,
        HarnessModelsSource::Config => wire::harness::HarnessModelsSource::Config,
        HarnessModelsSource::Merged => wire::harness::HarnessModelsSource::Merged,
        HarnessModelsSource::Static => wire::harness::HarnessModelsSource::Static,
    }
}

fn effort_option_body(value: EffortOption) -> wire::harness::EffortOption {
    wire::harness::EffortOption {
        id: value.id,
        label: value.label,
        is_default: value.is_default,
    }
}
