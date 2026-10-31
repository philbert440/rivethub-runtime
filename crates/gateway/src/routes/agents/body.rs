use kernel::agent_presets;
use wire::gateway::agents;
use wire::gateway::agents::KnownHarness;
use wire::harness::HarnessId;

pub(super) fn preset_body(preset: agent_presets::AgentPreset) -> agents::AgentPreset {
    agents::AgentPreset {
        id: preset.id,
        name: preset.name,
        color: preset.color,
        harness_id: preset.harness_id,
        model: preset.model,
        effort: preset.effort,
        system_prompt: preset.system_prompt,
        node: preset.node,
        directory: preset.directory,
        shared_link: preset.shared_link,
        node_base_url: preset.node_base_url,
        sort_order: preset.sort_order,
        created_at: preset.created_at,
        updated_at: preset.updated_at,
        extra: preset.extra,
    }
}

pub(super) fn preset(body: agents::AgentPreset) -> agent_presets::AgentPreset {
    agent_presets::AgentPreset {
        id: body.id,
        name: body.name,
        color: body.color,
        harness_id: body.harness_id,
        model: body.model,
        effort: body.effort,
        system_prompt: body.system_prompt,
        node: body.node,
        directory: body.directory,
        shared_link: body.shared_link,
        node_base_url: body.node_base_url,
        sort_order: body.sort_order,
        created_at: body.created_at,
        updated_at: body.updated_at,
        extra: body.extra,
    }
}

pub(super) fn update(body: agents::AgentPresetUpdate) -> agent_presets::AgentPresetUpdate {
    agent_presets::AgentPresetUpdate {
        name: body.name,
        color: body.color,
        harness_id: body.harness_id.map(|harness| harness.map(HarnessId::from)),
        model: body.model,
        effort: body.effort,
        system_prompt: body.system_prompt,
        node: body.node,
        directory: body.directory,
        shared_link: body.shared_link,
        node_base_url: body.node_base_url,
        sort_order: body.sort_order,
    }
}

pub(super) fn update_body(update: agent_presets::AgentPresetUpdate) -> agents::AgentPresetUpdate {
    agents::AgentPresetUpdate {
        name: update.name,
        color: update.color,
        harness_id: update
            .harness_id
            .map(|harness| harness.map(KnownHarness::from)),
        model: update.model,
        effort: update.effort,
        system_prompt: update.system_prompt,
        node: update.node,
        directory: update.directory,
        shared_link: update.shared_link,
        node_base_url: update.node_base_url,
        sort_order: update.sort_order,
    }
}

pub(super) const fn backend_body(
    backend: agent_presets::AgentStoreBackend,
) -> agents::AgentStoreBackend {
    match backend {
        agent_presets::AgentStoreBackend::Postgres => agents::AgentStoreBackend::Postgres,
        agent_presets::AgentStoreBackend::File => agents::AgentStoreBackend::File,
    }
}
