use super::*;

fn tree_config() -> Config {
    let mut config = Config::default();
    config.ui.sidebar.mode = crate::config::SidebarModeConfig::Tree;
    config
}

/// One project with an agent pane and a shell pane in tab 1, plus a second tab.
fn tree_snapshot() -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.workspaces[0].label = "Job search".into();
    let mut second_tab = projected.tabs[0].clone();
    second_tab.tab_id = "tab_2".into();
    second_tab.number = 2;
    second_tab.label = "2".into();
    second_tab.focused = false;
    projected.tabs.push(second_tab);
    let mut shell_pane = projected.panes[0].clone();
    shell_pane.pane_id = "pane_2".into();
    shell_pane.label = Some("bash".into());
    shell_pane.focused = false;
    projected.panes.push(shell_pane);
    let mut other_pane = projected.panes[0].clone();
    other_pane.pane_id = "pane_3".into();
    other_pane.tab_id = "tab_2".into();
    other_pane.focused = false;
    projected.panes.push(other_pane);
    projected.agents = vec![ClientShellAgent {
        pane_id: "pane_1".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: None,
        display_agent: None,
        agent: Some("claude".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Working,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: true,
    }];
    projected
}

fn tree_state(config: &Config) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(config));
    state.set_snapshot(Box::new(tree_snapshot()));
    state.set_pane_surface(surface());
    state
}

fn sidebar_text(frame: &FrameData, width: usize) -> String {
    frame_rows(frame)
        .into_iter()
        .map(|row| row.chars().take(width).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn click(state: &mut ClientShellState, rect: Rect) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })])
}

#[test]
fn default_sidebar_keeps_the_agents_section() {
    let mut state = tree_state(&Config::default());
    let frame = state.compose(106, 30).expect("flat sidebar frame");
    let text = sidebar_text(&frame, 26);
    assert!(text.contains(" spaces"), "{text}");
    assert!(text.contains(" agents"), "{text}");
    assert!(!state.hits.agents.is_empty());
    assert!(state.hits.tree_toggles.is_empty());
    assert!(state.hits.tree_targets.is_empty());
}

#[test]
fn tree_mode_renders_projects_tabs_and_panes_without_agents_section() {
    let mut state = tree_state(&tree_config());
    let frame = state.compose(106, 30).expect("tree sidebar frame");
    let text = sidebar_text(&frame, 26);
    assert!(text.contains(" PROJECTS"), "{text}");
    assert!(text.contains("▾ Job search"), "{text}");
    assert!(text.contains("▾ 1   /repo"), "{text}");
    assert!(text.contains("├ claude"), "{text}");
    assert!(text.contains("● run"), "{text}");
    assert!(text.contains("└ bash"), "{text}");
    assert!(text.contains("+ new project"), "{text}");
    assert!(!text.contains(" agents"), "{text}");
    assert!(state.hits.agents.is_empty());
    assert_eq!(state.hits.agent_body, Rect::default());
    assert_eq!(state.hits.sidebar_section_divider, Rect::default());
    assert_eq!(state.hits.workspaces.len(), 1);
    assert_eq!(state.hits.tree_toggles.len(), 3);
    assert_eq!(state.hits.tree_targets.len(), 5);
}

#[test]
fn clicking_tree_rows_focuses_tabs_and_panes() {
    let mut state = tree_state(&tree_config());
    state.compose(106, 30).expect("tree sidebar frame");
    let pane_row = state
        .hits
        .tree_targets
        .iter()
        .find(|(_, target)| *target == ClientEndpointFocusTarget::Pane("pane_2".into()))
        .expect("shell pane row")
        .0;
    let outcome = click(&mut state, Rect::new(pane_row.x + 10, pane_row.y, 1, 1));
    assert!(outcome.actions.iter().any(|action| matches!(
        action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(
                &request.method,
                crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_2"
            )
    )));

    let tab_row = state
        .hits
        .tree_targets
        .iter()
        .find(|(_, target)| *target == ClientEndpointFocusTarget::Tab("tab_2".into()))
        .expect("second tab row")
        .0;
    let outcome = click(&mut state, Rect::new(tab_row.x + 8, tab_row.y, 1, 1));
    assert!(outcome.actions.iter().any(|action| matches!(
        action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(
                &request.method,
                crate::api::schema::Method::TabFocus(target) if target.tab_id == "tab_2"
            )
    )));
}

#[test]
fn tree_toggles_close_rows_and_survive_restart() {
    let path = std::env::temp_dir().join(format!(
        "herdr-tree-sidebar-preferences-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let config = tree_config();
    let mut state = ClientShellState::new(
        ClientShellConfig::from_config(&config).with_preferences_path(path.clone()),
    );
    state.set_snapshot(Box::new(tree_snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("tree sidebar frame");

    let tab_toggle = state
        .hits
        .tree_toggles
        .iter()
        .find(|(_, id)| id == "tab_1")
        .expect("tab toggle")
        .0;
    let outcome = click(&mut state, tab_toggle);
    assert!(outcome.repaint);
    assert!(outcome.actions.is_empty(), "toggles must not change focus");
    assert!(state.tree_collapsed.contains("tab_1"));

    let frame = state.compose(106, 30).expect("closed tab frame");
    let text = sidebar_text(&frame, 26);
    assert!(text.contains("▸ 1"), "{text}");
    assert!(text.contains("(2)"), "{text}");
    assert!(!text.contains("claude"), "{text}");
    assert!(!state
        .hits
        .tree_targets
        .iter()
        .any(|(_, target)| *target == ClientEndpointFocusTarget::Pane("pane_1".into())));

    let mut restarted = ClientShellState::new(
        ClientShellConfig::from_config(&config).with_preferences_path(path.clone()),
    );
    assert!(restarted.tree_collapsed.contains("tab_1"));
    restarted.set_snapshot(Box::new(tree_snapshot()));
    restarted.set_pane_surface(surface());
    let frame = restarted.compose(106, 30).expect("restarted tree frame");
    assert!(sidebar_text(&frame, 26).contains("▸ 1"));
    std::fs::remove_file(path).expect("remove tree preferences");
}

#[test]
fn collapsed_sidebar_hides_agents_in_tree_mode() {
    let mut config = tree_config();
    config.ui.sidebar_start_collapsed = true;
    let mut state = tree_state(&config);
    state.compose(106, 30).expect("collapsed tree sidebar");
    assert!(state.hits.agents.is_empty());
    assert_eq!(state.hits.workspaces.len(), 1);
}
