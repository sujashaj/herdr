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
    assert!(state.hits.tree_targets.is_empty());
}

#[test]
fn tree_mode_renders_projects_tabs_and_panes_without_agents_section() {
    let mut state = tree_state(&tree_config());
    let frame = state.compose(106, 30).expect("tree sidebar frame");
    let text = sidebar_text(&frame, 26);
    assert!(text.contains(" PROJECTS"), "{text}");
    assert!(text.contains(" Job search"), "{text}");
    assert!(
        text.lines()
            .any(|row| row.trim_end_matches([' ', '│']) == " ├ ● ■ 1"),
        "{text}"
    );
    assert!(!text.contains("repo"), "tab rows show no folder: {text}");
    assert!(!text.contains('▾') && !text.contains('▸'), "{text}");
    assert!(text.contains("├ ● ✻ claude"), "{text}");
    let pane_row = text
        .lines()
        .find(|row| row.contains("├ ● ✻ claude"))
        .expect("claude pane row");
    // Pane branches hang from the tab's status dot (col 3), tab branches from the
    // project dot (col 1).
    assert_eq!(pane_row.chars().position(|ch| ch == '├'), Some(3), "{text}");
    assert_eq!(pane_row.chars().position(|ch| ch == '│'), Some(1), "{text}");
    let tab_row = text
        .lines()
        .find(|row| row.trim_end_matches([' ', '│']) == " ├ ● ■ 1")
        .expect("tab row");
    assert_eq!(tab_row.chars().position(|ch| ch == '●'), Some(3), "{text}");
    assert_eq!(tab_row.chars().position(|ch| ch == '■'), Some(5), "{text}");
    assert_eq!(tab_row.chars().position(|ch| ch == '1'), Some(7), "{text}");
    assert!(!text.contains("run"), "no status words: {text}");
    assert!(text.contains("└ >_  bash"), "{text}");
    assert!(
        text.lines()
            .any(|row| row.trim_end_matches([' ', '│']) == " └ · ■ 2"),
        "a tab without agents shows a small grey dot: {text}"
    );
    let project_row = text
        .lines()
        .find(|row| row.contains("Job search"))
        .expect("project row");
    assert!(
        project_row.contains("Job search ───"),
        "rule follows the project name: {project_row:?}"
    );
    let rule_y = frame_rows(&frame)
        .iter()
        .position(|row| row.contains("Job search"))
        .expect("project frame row");
    let rule_x = project_row
        .chars()
        .position(|ch| ch == '─')
        .expect("rule cell");
    assert_eq!(
        frame.cells[rule_y * frame.width as usize + rule_x].fg,
        crate::protocol::color_to_u32(state.config.palette.blue),
        "rule uses the project color"
    );
    assert!(text.contains("+ new project"), "{text}");
    assert!(!text.contains(" agents"), "{text}");
    assert!(state.hits.agents.is_empty());
    assert_eq!(state.hits.agent_body, Rect::default());
    assert_eq!(state.hits.sidebar_section_divider, Rect::default());
    assert_eq!(state.hits.workspaces.len(), 1);
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
fn collapsed_sidebar_hides_agents_in_tree_mode() {
    let mut config = tree_config();
    config.ui.sidebar_start_collapsed = true;
    let mut state = tree_state(&config);
    state.compose(106, 30).expect("collapsed tree sidebar");
    assert!(state.hits.agents.is_empty());
    assert_eq!(state.hits.workspaces.len(), 1);
}

#[test]
fn tree_mode_separates_projects_with_one_blank_row() {
    let mut projected = tree_snapshot();
    let mut second_workspace = projected.workspaces[0].clone();
    second_workspace.workspace_id = "ws_2".into();
    second_workspace.active_tab_id = "tab_3".into();
    second_workspace.number = 2;
    second_workspace.label = "Open source".into();
    second_workspace.focused = false;
    projected.workspaces.push(second_workspace);
    let mut third_tab = projected.tabs[0].clone();
    third_tab.tab_id = "tab_3".into();
    third_tab.workspace_id = "ws_2".into();
    third_tab.focused = false;
    projected.tabs.push(third_tab);
    let mut third_pane = projected.panes[0].clone();
    third_pane.pane_id = "pane_4".into();
    third_pane.workspace_id = "ws_2".into();
    third_pane.tab_id = "tab_3".into();
    third_pane.focused = false;
    projected.panes.push(third_pane);

    let config = tree_config();
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    let frame = state.compose(106, 30).expect("two project tree frame");
    let text = sidebar_text(&frame, 26);
    let rows = text.lines().collect::<Vec<_>>();
    let is_blank = |row: &str| row.chars().all(|ch| ch == ' ' || ch == '│');

    let first_header = rows
        .iter()
        .position(|row| row.contains(" Job search"))
        .expect("first project header");
    let second_header = rows
        .iter()
        .position(|row| row.contains(" Open source"))
        .expect("second project header");
    assert!(second_header >= 2, "{text}");
    assert!(is_blank(rows[second_header - 1]), "{text}");
    assert!(!is_blank(rows[second_header - 2]), "{text}");
    let blank_rows = rows[first_header..second_header]
        .iter()
        .filter(|row| is_blank(row))
        .count();
    assert_eq!(blank_rows, 1, "{text}");
    assert_eq!(state.hits.workspaces.len(), 2);
}

fn claude_row_with_status(status: AgentStatus) -> String {
    let config = tree_config();
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    let mut projected = tree_snapshot();
    projected.agents[0].agent_status = status;
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    let frame = state.compose(106, 30).expect("tree sidebar frame");
    frame_rows(&frame)
        .into_iter()
        .find(|row| row.contains("claude"))
        .expect("claude pane row")
        .chars()
        .take(26)
        .collect()
}

fn column_of(row: &str, needle: &str) -> usize {
    let byte = row
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} in {row:?}"));
    row[..byte].chars().count()
}

#[test]
fn status_dot_sits_between_the_branch_and_the_pane_icon() {
    for (status, dot) in [
        (AgentStatus::Working, "●"),
        (AgentStatus::Blocked, "●"),
        (AgentStatus::Idle, "○"),
        (AgentStatus::Done, "○"),
    ] {
        let row = claude_row_with_status(status);
        assert!(
            row.contains(&format!("├ {dot} ✻ claude")),
            "{status:?}: {row:?}"
        );
        for word in ["run", "blocked", "idle", "done"] {
            assert!(!row.contains(word), "{status:?}: {row:?}");
        }
        assert_eq!(column_of(&row, dot), 5, "{status:?}: {row:?}");
    }
}

#[test]
fn terminal_icon_starts_under_the_dot_and_pane_labels_align() {
    let mut state = tree_state(&tree_config());
    let frame = state.compose(106, 30).expect("tree sidebar frame");
    let rows = frame_rows(&frame);
    let agent_row = rows
        .iter()
        .find(|row| row.contains("claude"))
        .expect("agent pane row");
    let shell_row = rows
        .iter()
        .find(|row| row.contains("bash"))
        .expect("shell pane row");
    // A terminal's `>_` starts in the column of an agent pane's status dot.
    assert_eq!(column_of(agent_row, "●"), 5, "{agent_row:?}");
    assert_eq!(column_of(shell_row, ">_"), 5, "{shell_row:?}");
    assert_eq!(column_of(agent_row, "✻"), 7, "{agent_row:?}");
    assert!(shell_row.contains("└ >_  bash"), "{shell_row:?}");
    assert_eq!(column_of(agent_row, "claude"), 9, "{agent_row:?}");
    assert_eq!(column_of(shell_row, "bash"), 9, "{shell_row:?}");
}

#[test]
fn tree_footer_shows_full_menu_label_and_toggle() {
    let mut state = tree_state(&tree_config());
    let frame = state.compose(106, 30).expect("tree sidebar frame");
    let rows = frame_rows(&frame);
    let footer = &rows[usize::from(state.hits.new_workspace.y)];
    let sidebar = footer.chars().take(26).collect::<String>();
    assert!(sidebar.contains("+ new project"), "{sidebar:?}");
    assert!(sidebar.contains("menu «"), "{sidebar:?}");
    assert!(!sidebar.contains("men«"), "{sidebar:?}");
    let launcher = state.hits.global_launcher;
    assert_eq!(launcher.width, 4);
    assert!(
        launcher.right() < state.hits.sidebar_toggle.x,
        "{launcher:?}"
    );
}

fn workspace_created(workspace_id: &str, label: &str) -> crate::api::schema::ResponseResult {
    crate::api::schema::ResponseResult::WorkspaceCreated {
        workspace: crate::api::schema::WorkspaceInfo {
            workspace_id: workspace_id.into(),
            number: 2,
            label: label.into(),
            focused: true,
            pane_count: 1,
            tab_count: 1,
            active_tab_id: "tab_9".into(),
            agent_status: AgentStatus::Unknown,
            tokens: HashMap::new(),
            worktree: None,
        },
        tab: crate::api::schema::TabInfo {
            tab_id: "tab_9".into(),
            workspace_id: workspace_id.into(),
            number: 1,
            label: "1".into(),
            focused: true,
            pane_count: 1,
            agent_status: AgentStatus::Unknown,
        },
        root_pane: match pane_scroll_result(0, 0, 0) {
            crate::api::schema::ResponseResult::PaneInfo { pane } => pane,
            _ => unreachable!(),
        },
    }
}

/// Clicks "+ new project" and replies to the resulting `workspace.create` request.
fn create_project_from_footer(state: &mut ClientShellState, label: &str) {
    state.compose(106, 30).expect("tree sidebar frame");
    let footer = state.hits.new_workspace;
    let outcome = click(state, footer);
    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!(
            "new project should use the endpoint API: {:?}",
            outcome.actions
        );
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::WorkspaceCreate(params)
            if params.label.is_none() && params.focus
    ));
    assert!(
        state.overlay.is_none(),
        "no prompt before the project exists"
    );
    let (repaint, actions) =
        state.handle_endpoint_result("boot-1", &request.id, Ok(workspace_created("ws_9", label)));
    assert!(repaint);
    assert!(actions.is_empty());
}

#[test]
fn new_project_in_tree_mode_opens_name_prompt_prefilled_with_auto_name() {
    let mut state = tree_state(&tree_config());
    create_project_from_footer(&mut state, "home");
    assert!(matches!(
        state.overlay.as_ref(),
        Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            input,
            target: ClientRenameTarget::NewProject { workspace_id, auto_name },
            ..
        })) if input.as_str() == "home" && workspace_id == "ws_9" && auto_name == "home"
    ));
    let frame = state.compose(106, 30).expect("name prompt frame");
    assert!(frame_rows(&frame).join("\n").contains("name project"));

    // Typing replaces the prefilled name; Enter renames through workspace.rename.
    let typed = state.handle_input_bytes(b"Dotfiles\r");
    let [ClientShellAction::Endpoint { request, .. }] = &typed.actions[..] else {
        panic!("rename should use endpoint API: {:?}", typed.actions);
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::WorkspaceRename(params)
            if params.workspace_id == "ws_9" && params.label == "Dotfiles"
    ));
    assert!(state.overlay.is_none());
}

#[test]
fn accepting_or_dismissing_new_project_prompt_keeps_auto_name() {
    let mut state = tree_state(&tree_config());
    create_project_from_footer(&mut state, "job-search");
    let accepted = state.handle_input_bytes(b"\r");
    assert!(accepted.actions.is_empty(), "{:?}", accepted.actions);
    assert!(state.overlay.is_none());

    create_project_from_footer(&mut state, "job-search");
    let dismissed = state.handle_input_bytes(b"\x1b");
    assert!(dismissed.actions.is_empty(), "{:?}", dismissed.actions);
    assert!(state.overlay.is_none());
}

#[test]
fn default_sidebar_new_workspace_click_does_not_prompt_after_creation() {
    let mut state = tree_state(&Config::default());
    state.compose(106, 30).expect("flat sidebar frame");
    let footer = state.hits.new_workspace;
    let outcome = click(&mut state, footer);
    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!("new workspace should use the endpoint API");
    };
    state.handle_endpoint_result("boot-1", &request.id, Ok(workspace_created("ws_9", "home")));
    assert!(state.overlay.is_none());
}

#[test]
fn tab_square_uses_the_tab_color_whatever_the_status() {
    let mut state = tree_state(&tree_config());
    let frame = state.compose(106, 30).expect("tree sidebar frame");
    let rows = frame_rows(&frame);
    let squares = rows
        .iter()
        .enumerate()
        .filter_map(|(y, row)| {
            let x = row.chars().take(26).position(|ch| ch == '■')?;
            Some(frame.cells[y * frame.width as usize + x].fg)
        })
        .collect::<Vec<_>>();
    assert_eq!(squares.len(), 2, "{rows:?}");
    let expected = crate::protocol::color_to_u32(state.config.palette.mauve);
    assert!(squares.iter().all(|fg| *fg == expected), "{squares:?}");
}

#[test]
fn long_pane_names_end_with_an_ellipsis() {
    let mut projected = tree_snapshot();
    projected.panes[0].label = Some("Playwright extension and MCP setup for the job search".into());
    let mut state = tree_state(&tree_config());
    state.set_snapshot(Box::new(projected));
    let frame = state.compose(106, 30).expect("tree sidebar frame");
    let text = sidebar_text(&frame, 26);
    let row = text
        .lines()
        .find(|row| row.contains("✻ Playwright"))
        .unwrap_or_else(|| panic!("pane name row: {text}"));
    assert!(row.contains('…'), "{row:?}");
    assert!(!row.contains("job search"), "{row:?}");
}

fn titled_snapshot(title: &str) -> ClientShellSnapshot {
    let mut projected = tree_snapshot();
    projected.agents[0].terminal_title_stripped = Some(title.into());
    projected
}

/// `(pane_id, label)` of every `pane.rename` request in `outcome`.
fn pane_renames(outcome: &ClientShellInput) -> Vec<(String, Option<String>)> {
    outcome
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientShellAction::Endpoint { request, .. } => match &request.method {
                crate::api::schema::Method::PaneRename(params) => {
                    Some((params.pane_id.clone(), params.label.clone()))
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[test]
fn first_session_title_names_the_pane_exactly_once() {
    let mut state = tree_state(&tree_config());
    state.set_snapshot(Box::new(titled_snapshot("Resume handoff instructions")));
    let outcome = state.lock_first_session_titles();
    assert_eq!(
        pane_renames(&outcome),
        [(
            "pane_1".to_owned(),
            Some("Resume handoff instructions".to_owned())
        )]
    );
    assert_eq!(outcome.actions.len(), 1);

    // Until the rename lands, the pane keeps its agent name rather than the live title.
    let frame = state.compose(106, 30).expect("tree sidebar frame");
    let text = sidebar_text(&frame, 26);
    assert!(text.contains("✻ claude"), "{text}");
    assert!(!text.contains("Resume handoff"), "{text}");

    // While the request is in flight, a repeat snapshot sends nothing.
    state.set_snapshot(Box::new(titled_snapshot("Resume handoff instructions")));
    assert!(pane_renames(&state.lock_first_session_titles()).is_empty());
}

#[test]
fn later_session_titles_never_rename_the_pane_again() {
    let mut state = tree_state(&tree_config());
    state.set_snapshot(Box::new(titled_snapshot("Resume handoff instructions")));
    assert_eq!(pane_renames(&state.lock_first_session_titles()).len(), 1);

    // The rename landed: the pane now carries the locked name.
    let mut landed = titled_snapshot("Cover letter for Acme");
    landed.panes[0].label = Some("Resume handoff instructions".into());
    state.set_snapshot(Box::new(landed));
    assert!(pane_renames(&state.lock_first_session_titles()).is_empty());
    let frame = state.compose(106, 30).expect("tree sidebar frame");
    let text = sidebar_text(&frame, 26);
    assert!(text.contains("Resume handoff"), "{text}");
    assert!(!text.contains("Cover letter"), "{text}");

    // Even with the label gone (the user cleared it), the new title is not locked in.
    let cleared = titled_snapshot("Cover letter for Acme");
    state.set_snapshot(Box::new(cleared));
    assert!(pane_renames(&state.lock_first_session_titles()).is_empty());
}

#[test]
fn a_later_title_is_not_locked_in_before_the_first_rename_lands() {
    let mut state = tree_state(&tree_config());
    state.set_snapshot(Box::new(titled_snapshot("Resume handoff instructions")));
    assert_eq!(pane_renames(&state.lock_first_session_titles()).len(), 1);
    let retitled = titled_snapshot("Cover letter for Acme");
    state.set_snapshot(Box::new(retitled));
    assert!(pane_renames(&state.lock_first_session_titles()).is_empty());
}

#[test]
fn generic_session_titles_do_not_name_the_pane() {
    let mut state = tree_state(&tree_config());
    for title in ["Claude Code", "claude", "  ", ""] {
        state.set_snapshot(Box::new(titled_snapshot(title)));
        assert!(
            pane_renames(&state.lock_first_session_titles()).is_empty(),
            "{title:?}"
        );
    }
    // No title at all.
    state.set_snapshot(Box::new(tree_snapshot()));
    assert!(pane_renames(&state.lock_first_session_titles()).is_empty());

    // The first real title still names the pane afterwards.
    state.set_snapshot(Box::new(titled_snapshot("Resume handoff instructions")));
    assert_eq!(pane_renames(&state.lock_first_session_titles()).len(), 1);
}

#[test]
fn named_panes_keep_their_name() {
    let mut state = tree_state(&tree_config());
    let mut named = titled_snapshot("Resume handoff instructions");
    named.panes[0].label = Some("mine".into());
    state.set_snapshot(Box::new(named));
    assert!(pane_renames(&state.lock_first_session_titles()).is_empty());

    // Clearing the name later does not bring an automatic one back.
    let cleared = titled_snapshot("Resume handoff instructions");
    state.set_snapshot(Box::new(cleared));
    assert!(pane_renames(&state.lock_first_session_titles()).is_empty());
}

#[test]
fn default_sidebar_never_names_panes() {
    let mut state = tree_state(&Config::default());
    state.set_snapshot(Box::new(titled_snapshot("Resume handoff instructions")));
    let outcome = state.lock_first_session_titles();
    assert!(outcome.actions.is_empty());
    assert!(state.settled_pane_names.is_empty());
}

#[test]
fn agent_pane_without_a_status_keeps_its_icon_in_place() {
    let settled = claude_row_with_status(AgentStatus::Working);
    let starting = claude_row_with_status(AgentStatus::Unknown);
    assert_eq!(
        column_of(&starting, "○"),
        5,
        "grey ring holds the dot slot: {starting:?}"
    );
    assert_eq!(
        column_of(&starting, "✻"),
        column_of(&settled, "✻"),
        "{starting:?} vs {settled:?}"
    );
}
