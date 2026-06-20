use crate::*;

#[tauri::command]
pub(super) async fn list_conversations(
    state: State<'_, DesktopState>,
    input: WorkspaceInput,
) -> std::result::Result<Vec<ConversationSummary>, String> {
    let workspace_root =
        normalize_workspace_root(&input.workspace_path).map_err(error_to_string)?;
    state
        .store
        .list_conversations(&workspace_root.display().to_string())
        .map_err(error_to_string)
}

#[tauri::command]
pub(super) async fn create_conversation(
    state: State<'_, DesktopState>,
    input: WorkspaceInput,
) -> std::result::Result<WorkspaceBootstrap, String> {
    let workspace_root =
        normalize_workspace_root(&input.workspace_path).map_err(error_to_string)?;
    state
        .store
        .create_conversation(
            &workspace_root.display().to_string(),
            &state.default_model,
            &state.system_prompt,
        )
        .map_err(error_to_string)?;
    state
        .store
        .bootstrap_workspace(&workspace_root, &state.default_model, &state.system_prompt)
        .map_err(error_to_string)
}

#[tauri::command]
pub(super) async fn load_conversation(
    state: State<'_, DesktopState>,
    input: ConversationInput,
) -> std::result::Result<SavedConversation, String> {
    let workspace_root =
        normalize_workspace_root(&input.workspace_path).map_err(error_to_string)?;
    state
        .store
        .load_conversation(
            &workspace_root.display().to_string(),
            &input.conversation_id,
        )
        .map_err(error_to_string)?
        .ok_or_else(|| "conversation not found".to_string())
}

#[tauri::command]
pub(super) async fn rename_conversation(
    state: State<'_, DesktopState>,
    input: RenameConversationInput,
) -> std::result::Result<Vec<ConversationSummary>, String> {
    let workspace_root =
        normalize_workspace_root(&input.workspace_path).map_err(error_to_string)?;
    let title = input.title.trim();
    if title.is_empty() {
        return Err("title cannot be empty".into());
    }
    let workspace_id = workspace_root.display().to_string();
    state
        .store
        .rename_conversation(&workspace_id, &input.conversation_id, title)
        .map_err(error_to_string)?;
    state
        .store
        .list_conversations(&workspace_id)
        .map_err(error_to_string)
}

#[tauri::command]
pub(super) async fn delete_conversation(
    state: State<'_, DesktopState>,
    input: ConversationInput,
) -> std::result::Result<WorkspaceBootstrap, String> {
    let workspace_root =
        normalize_workspace_root(&input.workspace_path).map_err(error_to_string)?;
    let workspace_id = workspace_root.display().to_string();
    {
        let active_turns = state.active_turns.lock().await;
        if active_turns.contains_key(&input.conversation_id) {
            return Err("a turn is already running for this conversation".into());
        }
    }
    state
        .store
        .delete_conversation(&workspace_id, &input.conversation_id)
        .map_err(error_to_string)?;
    state
        .store
        .bootstrap_workspace(&workspace_root, &state.default_model, &state.system_prompt)
        .map_err(error_to_string)
}

#[tauri::command]
pub(super) async fn set_conversation_mode(
    state: State<'_, DesktopState>,
    input: ConversationModeInput,
) -> std::result::Result<SavedConversation, String> {
    let workspace_root =
        normalize_workspace_root(&input.workspace_path).map_err(error_to_string)?;
    let workspace_id = workspace_root.display().to_string();
    {
        let active_turns = state.active_turns.lock().await;
        if active_turns.contains_key(&input.conversation_id) {
            return Err("a turn is already running for this conversation".into());
        }
    }

    let mut conversation = state
        .store
        .load_conversation(&workspace_id, &input.conversation_id)
        .map_err(error_to_string)?
        .ok_or_else(|| "conversation not found".to_string())?;

    let mode = AgentMode::from(input.mode);
    let current_plan_workflow = std::mem::take(&mut conversation.plan_workflow);
    conversation.plan_workflow = match mode {
        AgentMode::Act => PlanWorkflowState::Idle,
        AgentMode::Plan => match current_plan_workflow {
            PlanWorkflowState::Idle => PlanWorkflowState::PlanningQuestions,
            current => current,
        },
        AgentMode::Goal => PlanWorkflowState::Idle,
    };
    conversation.goal_workflow = match mode {
        AgentMode::Goal => resume_goal_workflow(std::mem::take(&mut conversation.goal_workflow)),
        AgentMode::Act | AgentMode::Plan => {
            pause_goal_workflow(std::mem::take(&mut conversation.goal_workflow))
        }
    };
    conversation.model = conversation.mode_model_settings.get(mode).clone();

    state
        .store
        .save_conversation(&conversation)
        .map_err(error_to_string)?;
    Ok(conversation)
}

#[tauri::command]
pub(super) async fn set_conversation_model_preference(
    state: State<'_, DesktopState>,
    input: ConversationModelPreferenceInput,
) -> std::result::Result<ModeModelSettings, String> {
    let workspace_root =
        normalize_workspace_root(&input.workspace_path).map_err(error_to_string)?;
    let workspace_id = workspace_root.display().to_string();
    let conversation_id = input.conversation_id;
    let mode = AgentMode::from(input.mode);

    {
        let active_turns = state.active_turns.lock().await;
        if active_turns.contains_key(&conversation_id) {
            return Err("a turn is already running for this conversation".into());
        }
    }

    let mut conversation = state
        .store
        .load_conversation(&workspace_id, &conversation_id)
        .map_err(error_to_string)?
        .ok_or_else(|| "conversation not found".to_string())?;
    let selected = model_with_optional_selection(
        conversation.mode_model_settings.get(mode),
        input.model,
        input.thinking,
    );
    let provider = provider_from_registry(&state, &selected.provider)?;
    provider
        .capabilities(&selected)
        .ok_or_else(|| format!("model `{}` is not supported", selected.name))?;

    conversation.mode_model_settings.set(mode, selected.clone());
    if conversation_active_mode(&conversation) == mode {
        conversation.model = selected.clone();
    }

    let mut default_settings = state
        .store
        .load_mode_model_settings(&state.default_model)
        .map_err(error_to_string)?;
    default_settings.set(mode, selected);

    state
        .store
        .save_conversation_and_mode_model_settings(&conversation, &default_settings)
        .map_err(error_to_string)?;
    Ok(conversation.mode_model_settings)
}

#[tauri::command]
pub(super) async fn list_mcp_settings(
    state: State<'_, DesktopState>,
) -> std::result::Result<McpSettings, String> {
    state.store.load_mcp_settings().map_err(error_to_string)
}

/// Return the global default mode/model assignment without requiring a
/// workspace to be open. Used by Welcome-screen flows (e.g. the migration
/// dialog) so the user can see which model will run an upcoming agentic
/// turn before they pick a workspace.
#[tauri::command]
pub(super) async fn list_default_mode_model_settings(
    state: State<'_, DesktopState>,
) -> std::result::Result<sinew_app::ModeModelSettings, String> {
    state
        .store
        .load_mode_model_settings(&state.default_model)
        .map_err(error_to_string)
}

/// Read the saved shell preference without requiring a workspace. The
/// Welcome screen surfaces this so a brand-new user can see (and switch)
/// it before they ever open a folder.
#[tauri::command]
pub(super) async fn get_shell_preference(
    state: State<'_, DesktopState>,
) -> std::result::Result<sinew_app::ShellPreference, String> {
    state
        .store
        .load_tool_settings()
        .map(|settings| settings.shell_preference)
        .map_err(error_to_string)
}

/// Persist a new shell preference (without going through the workspace
/// catalog flow) and apply it process-wide so the next bash-tool turn
/// and any newly spawned interactive terminal honor it immediately.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetShellPreferenceInput {
    pub shell_preference: sinew_app::ShellPreference,
}

#[tauri::command]
pub(super) async fn set_shell_preference(
    state: State<'_, DesktopState>,
    input: SetShellPreferenceInput,
) -> std::result::Result<sinew_app::ShellPreference, String> {
    let mut settings = state
        .store
        .load_tool_settings()
        .map_err(error_to_string)?;
    settings.shell_preference = input.shell_preference;
    state
        .store
        .save_tool_settings(&settings)
        .map_err(error_to_string)?;
    sinew_app::set_shell_preference(input.shell_preference);
    Ok(input.shell_preference)
}

#[tauri::command]
pub(super) async fn save_mcp_settings(
    state: State<'_, DesktopState>,
    input: SaveMcpSettingsInput,
) -> std::result::Result<McpSettings, String> {
    state
        .store
        .save_mcp_settings(&input.settings)
        .map_err(error_to_string)?;
    Ok(input.settings)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ImportMcpServersInput {
    pub file_path: String,
    /// `claude` for a `.claude.json` (JSON) or `codex` for a Codex CLI
    /// `config.toml`. Other values are rejected.
    pub format: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ImportMcpServersOutput {
    pub settings: sinew_app::McpSettings,
    pub result: sinew_app::ImportMcpResult,
}

/// Parse the given Claude Code or Codex CLI MCP config file and merge
/// the servers it defines into Sinew's own MCP settings (duplicates by
/// name are skipped so a re-run is a no-op). Returns the merged
/// settings + a summary of what was added vs skipped.
#[tauri::command]
pub(super) async fn import_mcp_servers_command(
    state: State<'_, DesktopState>,
    input: ImportMcpServersInput,
) -> std::result::Result<ImportMcpServersOutput, String> {
    let format = sinew_app::McpImportFormat::parse(&input.format).map_err(error_to_string)?;
    let path = std::path::PathBuf::from(&input.file_path);
    let imported =
        sinew_app::parse_mcp_import_file(&path, format).map_err(error_to_string)?;
    let current = state.store.load_mcp_settings().map_err(error_to_string)?;
    let (merged, result) =
        sinew_app::merge_imported_mcp_servers(&current, imported, path.display().to_string());
    let saved = state
        .store
        .save_mcp_settings(&merged)
        .map_err(error_to_string)?;
    Ok(ImportMcpServersOutput {
        settings: saved,
        result,
    })
}

#[tauri::command]
pub(super) async fn list_tool_settings(
    state: State<'_, DesktopState>,
    input: WorkspaceInput,
) -> std::result::Result<ToolSettingsView, String> {
    let workspace_root =
        normalize_workspace_root(&input.workspace_path).map_err(error_to_string)?;
    let settings = state.store.load_tool_settings().map_err(error_to_string)?;
    Ok(tool_settings_view(
        &settings,
        &configurable_tool_catalog(&workspace_root),
    ))
}

#[tauri::command]
pub(super) async fn save_tool_settings(
    state: State<'_, DesktopState>,
    input: SaveToolSettingsInput,
) -> std::result::Result<ToolSettingsView, String> {
    let workspace_root =
        normalize_workspace_root(&input.workspace_path).map_err(error_to_string)?;
    let catalog = configurable_tool_catalog(&workspace_root);
    let saved = state
        .store
        .save_tool_settings_for_catalog(&input.settings, &catalog)
        .map_err(error_to_string)?;
    // Apply the (possibly changed) shell preference immediately so the next
    // turn and any newly spawned interactive terminal pick it up without a
    // restart.
    sinew_app::set_shell_preference(saved.shell_preference);
    Ok(tool_settings_view(&saved, &catalog))
}

#[tauri::command]
pub(super) async fn list_sub_agent_settings(
    state: State<'_, DesktopState>,
) -> std::result::Result<SubAgentSettings, String> {
    state
        .store
        .load_sub_agent_settings()
        .map_err(error_to_string)
}

#[tauri::command]
pub(super) async fn save_sub_agent_settings(
    state: State<'_, DesktopState>,
    input: SaveSubAgentSettingsInput,
) -> std::result::Result<SubAgentSettings, String> {
    for agent in input.settings.agents.iter().filter(|agent| agent.enabled) {
        let provider = provider_from_registry(&state, &agent.model.provider)?;
        provider
            .capabilities(&agent.model)
            .ok_or_else(|| format!("model `{}` is not supported", agent.model.name))?;
    }
    state
        .store
        .save_sub_agent_settings(&input.settings)
        .map_err(error_to_string)
}

#[tauri::command]
pub(super) async fn probe_mcp_tools(
    state: State<'_, DesktopState>,
) -> std::result::Result<Vec<sinew_app::McpServerProbe>, String> {
    let settings = state.store.load_mcp_settings().map_err(error_to_string)?;
    Ok(probe_mcp_servers(&settings).await)
}

#[tauri::command]
pub(super) async fn list_installed_skills_command(
    state: State<'_, DesktopState>,
    input: WorkspaceInput,
) -> std::result::Result<Vec<InstalledSkill>, String> {
    let workspace_root =
        normalize_workspace_root(&input.workspace_path).map_err(error_to_string)?;
    let settings = state.store.load_skill_settings().map_err(error_to_string)?;
    Ok(list_installed_skills(workspace_root, &settings))
}

#[tauri::command]
pub(super) async fn save_skill_settings(
    state: State<'_, DesktopState>,
    input: SaveSkillSettingsInput,
) -> std::result::Result<Vec<InstalledSkill>, String> {
    let workspace_root =
        normalize_workspace_root(&input.workspace_path).map_err(error_to_string)?;
    let saved = state
        .store
        .save_skill_settings(&input.settings)
        .map_err(error_to_string)?;
    Ok(list_installed_skills(workspace_root, &saved))
}
