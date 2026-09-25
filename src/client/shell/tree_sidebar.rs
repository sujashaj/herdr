//! Tree presentation of the expanded sidebar: Project → Tab → Pane.
//!
//! Rows are built from the client snapshot only. Building borrows snapshot strings and keeps
//! collapse state as a set of workspace/tab IDs, so a frame costs one pass over the snapshot
//! and no filesystem or process work.

use std::collections::{HashMap, HashSet};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};

use super::render::{display_width, put_right_text, put_text, ShellRenderState};
use super::*;
use crate::api::schema::AgentStatus;
use crate::protocol::{ClientShellAgent, ClientShellPane};

const PROJECT_INDENT: u16 = 1;
const TAB_INDENT: u16 = 3;
const PANE_INDENT: u16 = 7;
const TOGGLE_WIDTH: u16 = 2;
const FALLBACK_PANE_LABEL: &str = "shell";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TreeRowKind {
    Project,
    Tab,
    Pane,
}

/// Folder shown next to a tab. The home prefix is rendered as `~` without allocating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TreePath<'a> {
    pub(super) tilde: bool,
    pub(super) rest: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TreeRow<'a> {
    pub(super) kind: TreeRowKind,
    /// Workspace, tab, or pane ID. Workspace and tab IDs double as collapse keys.
    pub(super) id: &'a str,
    pub(super) label: &'a str,
    pub(super) path: Option<TreePath<'a>>,
    pub(super) collapsed: bool,
    /// Panes hidden below a closed project or tab.
    pub(super) pane_count: usize,
    /// Agent status for pane rows. `None` when the pane has no agent or its state is unknown.
    pub(super) status: Option<AgentStatus>,
    pub(super) focused: bool,
    pub(super) last_child: bool,
}

impl TreeRow<'_> {
    pub(super) fn focus_target(&self) -> ClientEndpointFocusTarget {
        match self.kind {
            TreeRowKind::Project => ClientEndpointFocusTarget::Workspace(self.id.to_owned()),
            TreeRowKind::Tab => ClientEndpointFocusTarget::Tab(self.id.to_owned()),
            TreeRowKind::Pane => ClientEndpointFocusTarget::Pane(self.id.to_owned()),
        }
    }

    fn collapsible(&self) -> bool {
        self.kind != TreeRowKind::Pane
    }
}

pub(super) fn shorten_home<'a>(path: &'a str, home: Option<&str>) -> TreePath<'a> {
    let home = home.map(|home| home.trim_end_matches(['/', '\\']));
    if let Some(home) = home.filter(|home| !home.is_empty()) {
        if let Some(rest) = path.strip_prefix(home) {
            if rest.is_empty() || rest.starts_with(['/', '\\']) {
                return TreePath { tilde: true, rest };
            }
        }
    }
    TreePath {
        tilde: false,
        rest: path,
    }
}

fn pane_label<'a>(pane: &'a ClientShellPane, agent: Option<&'a ClientShellAgent>) -> &'a str {
    pane.label
        .as_deref()
        .or_else(|| agent.and_then(|agent| agent.name.as_deref()))
        .or_else(|| agent.and_then(|agent| agent.agent.as_deref()))
        .or_else(|| agent.and_then(|agent| agent.display_agent.as_deref()))
        .filter(|label| !label.trim().is_empty())
        .unwrap_or(FALLBACK_PANE_LABEL)
}

fn pane_path(pane: &ClientShellPane) -> Option<&str> {
    pane.foreground_cwd
        .as_deref()
        .or(pane.cwd.as_deref())
        .filter(|path| !path.is_empty())
}

/// Builds the visible tree rows in snapshot order.
pub(super) fn tree_rows<'a>(
    snapshot: &'a ClientShellSnapshot,
    collapsed: &HashSet<String>,
    home: Option<&str>,
) -> Vec<TreeRow<'a>> {
    let mut panes_by_tab = HashMap::<&str, Vec<&ClientShellPane>>::new();
    for pane in &snapshot.panes {
        panes_by_tab
            .entry(pane.tab_id.as_str())
            .or_default()
            .push(pane);
    }
    let agents = snapshot
        .agents
        .iter()
        .map(|agent| (agent.pane_id.as_str(), agent))
        .collect::<HashMap<_, _>>();
    let tab_pane_count = |tab_id: &str| panes_by_tab.get(tab_id).map_or(0, Vec::len);

    let mut rows =
        Vec::with_capacity(snapshot.workspaces.len() + snapshot.tabs.len() + snapshot.panes.len());
    for workspace in &snapshot.workspaces {
        let tabs = || {
            snapshot
                .tabs
                .iter()
                .filter(move |tab| tab.workspace_id == workspace.workspace_id)
        };
        let project_collapsed = collapsed.contains(&workspace.workspace_id);
        rows.push(TreeRow {
            kind: TreeRowKind::Project,
            id: &workspace.workspace_id,
            label: &workspace.label,
            path: None,
            collapsed: project_collapsed,
            pane_count: tabs().map(|tab| tab_pane_count(&tab.tab_id)).sum(),
            status: None,
            focused: workspace.focused,
            last_child: false,
        });
        if project_collapsed {
            continue;
        }
        for tab in tabs() {
            let panes = panes_by_tab
                .get(tab.tab_id.as_str())
                .map(Vec::as_slice)
                .unwrap_or_default();
            let tab_collapsed = collapsed.contains(&tab.tab_id);
            rows.push(TreeRow {
                kind: TreeRowKind::Tab,
                id: &tab.tab_id,
                label: &tab.label,
                path: panes
                    .iter()
                    .find(|pane| pane.focused)
                    .or(panes.first())
                    .and_then(|pane| pane_path(pane))
                    .map(|path| shorten_home(path, home)),
                collapsed: tab_collapsed,
                pane_count: panes.len(),
                status: None,
                focused: tab.focused,
                last_child: false,
            });
            if tab_collapsed {
                continue;
            }
            for (index, pane) in panes.iter().enumerate() {
                let agent = agents.get(pane.pane_id.as_str()).copied();
                rows.push(TreeRow {
                    kind: TreeRowKind::Pane,
                    id: &pane.pane_id,
                    label: pane_label(pane, agent),
                    path: None,
                    collapsed: false,
                    pane_count: 1,
                    status: agent
                        .map(|agent| agent.agent_status)
                        .filter(|status| *status != AgentStatus::Unknown),
                    focused: pane.focused,
                    last_child: index + 1 == panes.len(),
                });
            }
        }
    }
    rows
}

/// The deepest visible row on the focus path, so a closed parent still shows where focus is.
pub(super) fn focused_row_index(rows: &[TreeRow<'_>]) -> Option<usize> {
    let mut focused = None;
    for (index, row) in rows.iter().enumerate() {
        match row.kind {
            TreeRowKind::Project => {
                if focused.is_some() {
                    break;
                }
                if row.focused {
                    focused = Some(index);
                }
            }
            TreeRowKind::Tab | TreeRowKind::Pane if focused.is_some() && row.focused => {
                focused = Some(index);
            }
            TreeRowKind::Tab | TreeRowKind::Pane => {}
        }
    }
    focused
}

pub(super) fn status_mark(status: AgentStatus) -> Option<(&'static str, &'static str)> {
    match status {
        AgentStatus::Working => Some(("●", "run")),
        AgentStatus::Blocked => Some(("●", "blocked")),
        AgentStatus::Idle => Some(("○", "idle")),
        AgentStatus::Done => Some(("○", "done")),
        AgentStatus::Unknown => None,
    }
}

fn row_indent(kind: TreeRowKind) -> u16 {
    match kind {
        TreeRowKind::Project => PROJECT_INDENT,
        TreeRowKind::Tab => TAB_INDENT,
        TreeRowKind::Pane => PANE_INDENT,
    }
}

/// Clickable open/close glyph of a project or tab row.
pub(super) fn toggle_rect(row: &TreeRow<'_>, rect: Rect) -> Option<Rect> {
    if !row.collapsible() {
        return None;
    }
    let x = rect.x.saturating_add(row_indent(row.kind));
    (x < rect.right()).then(|| Rect::new(x, rect.y, TOGGLE_WIDTH.min(rect.right() - x), 1))
}

fn uniform_scroll_metrics(
    rows: usize,
    body_height: u16,
    requested_start: usize,
) -> crate::pane::ScrollMetrics {
    let body = usize::from(body_height);
    if rows == 0 || body == 0 {
        return crate::pane::ScrollMetrics {
            offset_from_bottom: 0,
            max_offset_from_bottom: 0,
            viewport_rows: 0,
        };
    }
    let max_start = rows.saturating_sub(body);
    let start = requested_start.min(max_start);
    crate::pane::ScrollMetrics {
        offset_from_bottom: max_start - start,
        max_offset_from_bottom: max_start,
        viewport_rows: body.min(rows - start),
    }
}

pub(super) fn render_tree_sidebar(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    state: &mut ShellRenderState<'_>,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    render::render_sidebar_background(buffer, area, palette);
    hits.sidebar_divider = if area.is_empty() {
        Rect::default()
    } else {
        Rect::new(area.right().saturating_sub(1), area.y, 1, area.height)
    };
    // Tree mode has no Agents section, so the project list owns the whole column.
    let content = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
    hits.sidebar_section_divider = Rect::default();
    put_text(
        buffer,
        content.x,
        content.y,
        content.width,
        " PROJECTS",
        Style::default()
            .fg(palette.overlay0)
            .add_modifier(Modifier::BOLD),
    );

    let rows = tree_rows(snapshot, state.tree_collapsed, config.home_dir.as_deref());
    let body = Rect::new(
        content.x,
        content.y.saturating_add(WORKSPACE_HEADER_ROWS),
        content.width,
        content.height.saturating_sub(WORKSPACE_HEADER_ROWS + 1),
    );
    hits.workspace_body = body;
    let focused_row = focused_row_index(&rows);
    if !body.is_empty() && std::mem::take(state.reveal_focused_workspace) {
        if let Some(target) = focused_row {
            let height = usize::from(body.height);
            if target < *state.workspace_scroll {
                *state.workspace_scroll = target;
            } else if target >= state.workspace_scroll.saturating_add(height) {
                *state.workspace_scroll = target + 1 - height;
            }
        }
    }
    let metrics = uniform_scroll_metrics(rows.len(), body.height, *state.workspace_scroll);
    hits.workspace_max_scroll = metrics.max_offset_from_bottom;
    hits.workspace_scroll_metrics = Some(metrics);
    *state.workspace_scroll = metrics
        .max_offset_from_bottom
        .saturating_sub(metrics.offset_from_bottom);
    let show_scrollbar = metrics.max_offset_from_bottom > 0 && body.width > 1;
    let content_width = body.width.saturating_sub(u16::from(show_scrollbar));

    for (offset, (index, row)) in rows
        .iter()
        .enumerate()
        .skip(*state.workspace_scroll)
        .take(usize::from(body.height))
        .enumerate()
    {
        let rect = Rect::new(body.x, body.y + offset as u16, content_width, 1);
        let selected = row.kind == TreeRowKind::Project
            && state
                .selected_workspace_id
                .is_some_and(|target| target.matches(state.active_endpoint_id, row.id));
        let dragged =
            row.kind == TreeRowKind::Project && state.dragged_workspace_id == Some(row.id);
        let background = if selected {
            Some(palette.selection_bg)
        } else if dragged {
            Some(palette.surface1)
        } else if focused_row == Some(index) {
            Some(sidebar::workspace_active_background(
                palette,
                state.selected_workspace_id.is_some(),
            ))
        } else {
            None
        };
        if let Some(background) = background {
            buffer.set_style(rect, Style::default().bg(background));
        }
        render_tree_row(buffer, rect, row, palette);

        let toggle = toggle_rect(row, rect);
        if let Some(toggle) = toggle {
            hits.tree_toggles.push((toggle, row.id.to_owned()));
        }
        if row.kind == TreeRowKind::Project {
            hits.workspaces.push(WorkspaceHit {
                rect,
                endpoint_id: ClientEndpointId::Local,
                workspace_id: row.id.to_owned(),
                indented: false,
                group_toggle: None,
            });
        } else {
            hits.tree_targets.push((rect, row.focus_target()));
        }
    }

    if show_scrollbar {
        let track = Rect::new(body.right().saturating_sub(1), body.y, 1, body.height);
        hits.workspace_scrollbar = track;
        super::scroll::render_list_scrollbar(buffer, track, metrics, palette);
    }

    if let Some(row) = state
        .workspace_drop_indicator_row
        .filter(|row| *row >= content.y.saturating_add(1) && *row < body.bottom())
    {
        put_text(
            buffer,
            body.x,
            row,
            body.width,
            &"─".repeat(body.width as usize),
            Style::default().fg(palette.accent),
        );
    }

    let footer_y = content.bottom().saturating_sub(1);
    if config.mouse_capture && content.height > 0 {
        let label = if content.width >= 19 {
            " + new project"
        } else {
            " + new"
        };
        let label_width = display_width(label).min(content.width);
        hits.new_workspace = Rect::new(content.x, footer_y, label_width, 1);
        put_text(
            buffer,
            content.x,
            footer_y,
            label_width,
            label,
            Style::default().fg(palette.overlay0),
        );
        let attention = super::global_menu::global_menu_attention(snapshot);
        let launcher_width = if attention { 6 } else { 4 };
        if content.width > label_width + launcher_width {
            hits.global_launcher = Rect::new(
                content.right().saturating_sub(launcher_width),
                footer_y,
                launcher_width,
                1,
            );
            put_right_text(
                buffer,
                content,
                footer_y,
                if attention { "● menu" } else { "menu" },
                if attention {
                    Style::default()
                        .fg(palette.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(palette.overlay0)
                },
            );
        }
    }

    hits.sidebar_toggle = Rect::new(
        area.right().saturating_sub(2),
        area.bottom().saturating_sub(1),
        u16::from(area.width > 1),
        u16::from(area.height > 0),
    );
    put_text(
        buffer,
        hits.sidebar_toggle.x,
        hits.sidebar_toggle.y,
        hits.sidebar_toggle.width,
        "«",
        Style::default().fg(palette.overlay0),
    );
}

fn render_tree_row(buffer: &mut Buffer, rect: Rect, row: &TreeRow<'_>, palette: &Palette) {
    let right = rect.right().saturating_sub(1);
    let mut x = rect.x.saturating_add(row_indent(row.kind));
    if x >= right {
        return;
    }

    // Right-hand annotation: `(n)` for closed rows, status dot and word for agent panes.
    let mut annotation_width = 0u16;
    if row.collapsed {
        let count = format!("({})", row.pane_count);
        annotation_width = display_width(&count);
        if x + annotation_width + 2 < right {
            put_text(
                buffer,
                right - annotation_width,
                rect.y,
                annotation_width,
                &count,
                Style::default().fg(palette.overlay0),
            );
        } else {
            annotation_width = 0;
        }
    } else if let Some((dot, word)) = row.status.and_then(status_mark) {
        let color = status_color(row.status.unwrap_or(AgentStatus::Unknown), palette);
        let full = 2 + display_width(word);
        let available = right.saturating_sub(x);
        let (width, text_word) = if available >= full + 6 {
            (full, true)
        } else if available >= 4 {
            (1, false)
        } else {
            (0, false)
        };
        if width > 0 {
            let start = right - width;
            put_text(buffer, start, rect.y, 1, dot, Style::default().fg(color));
            if text_word {
                put_text(
                    buffer,
                    start + 2,
                    rect.y,
                    width - 2,
                    word,
                    Style::default().fg(color),
                );
            }
            annotation_width = width;
        }
    }
    let text_right = if annotation_width > 0 {
        right.saturating_sub(annotation_width + 1)
    } else {
        right
    };

    let muted = Style::default().fg(palette.overlay0);
    match row.kind {
        TreeRowKind::Project | TreeRowKind::Tab => {
            let glyph = if row.collapsed { "▸" } else { "▾" };
            x = render::put_segment(buffer, x, rect.y, text_right, glyph, muted);
            x = render::put_segment(buffer, x, rect.y, text_right, " ", muted);
        }
        TreeRowKind::Pane => {
            let branch = if row.last_child { "└ " } else { "├ " };
            x = render::put_segment(buffer, x, rect.y, text_right, branch, muted);
        }
    }

    let label_style = match row.kind {
        TreeRowKind::Project => Style::default()
            .fg(if row.focused {
                palette.text
            } else {
                palette.subtext0
            })
            .add_modifier(Modifier::BOLD),
        TreeRowKind::Tab | TreeRowKind::Pane => Style::default().fg(if row.focused {
            palette.text
        } else {
            palette.subtext0
        }),
    };
    x = render::put_segment(buffer, x, rect.y, text_right, row.label, label_style);

    if let Some(path) = row.path {
        // Numeric tab labels line up their folders in one column.
        let pad = 4u16.saturating_sub(display_width(row.label)).max(1);
        x = x.saturating_add(pad).min(text_right);
        if path.tilde {
            x = render::put_segment(buffer, x, rect.y, text_right, "~", muted);
        }
        render::put_segment(buffer, x, rect.y, text_right, path.rest, muted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ClientShellTab;

    fn workspace(id: &str, label: &str, focused: bool) -> ClientShellWorkspace {
        ClientShellWorkspace {
            workspace_id: id.to_owned(),
            active_tab_id: format!("{id}:t1"),
            new_workspace_cwd: "/".to_owned(),
            number: 1,
            label: label.to_owned(),
            custom_label: true,
            branch: None,
            git_ahead_behind: None,
            tokens: Vec::new(),
            worktree: None,
            focused,
            agent_status: AgentStatus::Unknown,
        }
    }

    fn tab(workspace_id: &str, number: usize, focused: bool) -> ClientShellTab {
        ClientShellTab {
            tab_id: format!("{workspace_id}:t{number}"),
            workspace_id: workspace_id.to_owned(),
            number,
            label: number.to_string(),
            custom_label: false,
            zoomed: false,
            focused,
            agent_status: AgentStatus::Unknown,
        }
    }

    fn pane(tab_id: &str, number: usize, cwd: &str, focused: bool) -> ClientShellPane {
        let workspace_id = tab_id.split(':').next().unwrap_or_default();
        ClientShellPane {
            pane_id: format!("{workspace_id}:p{number}"),
            workspace_id: workspace_id.to_owned(),
            tab_id: tab_id.to_owned(),
            label: None,
            cwd: Some(cwd.to_owned()),
            foreground_cwd: None,
            focused,
            right_click_passthrough: false,
        }
    }

    fn agent(pane_id: &str, status: AgentStatus) -> ClientShellAgent {
        let mut parts = pane_id.split(':');
        let workspace_id = parts.next().unwrap_or_default().to_owned();
        ClientShellAgent {
            pane_id: pane_id.to_owned(),
            workspace_id: workspace_id.clone(),
            tab_id: format!("{workspace_id}:t1"),
            name: None,
            display_agent: Some("Claude Code".to_owned()),
            agent: Some("claude".to_owned()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: status,
            state_change_seq: 0,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: false,
        }
    }

    /// Two projects: "Job search" (tabs 1 and 2) and "OpenMRS" (tab 1).
    fn snapshot() -> ClientShellSnapshot {
        let mut snapshot = super::super::tests::snapshot();
        snapshot.workspaces = vec![
            workspace("w1", "Job search", true),
            workspace("w2", "OpenMRS", false),
        ];
        snapshot.tabs = vec![tab("w1", 1, true), tab("w1", 2, false), tab("w2", 1, false)];
        snapshot.panes = vec![
            pane("w1:t1", 1, "/home/suja/resume/suja", true),
            pane("w1:t1", 2, "/home/suja/resume/suja", false),
            pane("w1:t2", 3, "/home/suja/Downloads", false),
            pane("w1:t2", 4, "/home/suja/Downloads", false),
            pane("w2:t1", 1, "/opt/openmrs-core", false),
        ];
        snapshot.panes[1].label = Some("bash".to_owned());
        snapshot.agents = vec![
            agent("w1:p1", AgentStatus::Working),
            agent("w2:p1", AgentStatus::Idle),
        ];
        snapshot
    }

    fn outline(rows: &[TreeRow<'_>]) -> Vec<String> {
        rows.iter()
            .map(|row| {
                let indent = match row.kind {
                    TreeRowKind::Project => "",
                    TreeRowKind::Tab => "  ",
                    TreeRowKind::Pane => "    ",
                };
                let toggle = match (row.kind, row.collapsed) {
                    (TreeRowKind::Pane, _) => "",
                    (_, true) => "+ ",
                    (_, false) => "- ",
                };
                let count = if row.collapsed {
                    format!(" ({})", row.pane_count)
                } else {
                    String::new()
                };
                let status = row
                    .status
                    .and_then(status_mark)
                    .map(|(dot, word)| format!(" {dot} {word}"))
                    .unwrap_or_default();
                format!("{indent}{toggle}{}{count}{status}", row.label)
            })
            .collect()
    }

    #[test]
    fn open_tree_lists_projects_tabs_and_panes_in_snapshot_order() {
        let snapshot = snapshot();
        let rows = tree_rows(&snapshot, &HashSet::new(), Some("/home/suja"));
        assert_eq!(
            outline(&rows),
            [
                "- Job search",
                "  - 1",
                "    claude ● run",
                "    bash",
                "  - 2",
                "    shell",
                "    shell",
                "- OpenMRS",
                "  - 1",
                "    claude ○ idle",
            ]
        );
        assert_eq!(
            rows[1].path,
            Some(TreePath {
                tilde: true,
                rest: "/resume/suja"
            })
        );
        assert_eq!(
            rows[8].path,
            Some(TreePath {
                tilde: false,
                rest: "/opt/openmrs-core"
            })
        );
        assert!(!rows[2].last_child);
        assert!(rows[3].last_child);
    }

    #[test]
    fn closed_rows_hide_children_and_report_pane_counts() {
        let snapshot = snapshot();
        let collapsed = HashSet::from(["w1:t2".to_owned(), "w2".to_owned()]);
        let rows = tree_rows(&snapshot, &collapsed, None);
        assert_eq!(
            outline(&rows),
            [
                "- Job search",
                "  - 1",
                "    claude ● run",
                "    bash",
                "  + 2 (2)",
                "+ OpenMRS (1)",
            ]
        );
    }

    #[test]
    fn status_marks_follow_agent_state_and_skip_unknown() {
        assert_eq!(status_mark(AgentStatus::Working), Some(("●", "run")));
        assert_eq!(status_mark(AgentStatus::Blocked), Some(("●", "blocked")));
        assert_eq!(status_mark(AgentStatus::Idle), Some(("○", "idle")));
        assert_eq!(status_mark(AgentStatus::Done), Some(("○", "done")));
        assert_eq!(status_mark(AgentStatus::Unknown), None);

        let mut snapshot = snapshot();
        snapshot.agents[0].agent_status = AgentStatus::Unknown;
        let rows = tree_rows(&snapshot, &HashSet::new(), None);
        assert_eq!(rows[2].status, None);
    }

    #[test]
    fn home_shortening_requires_a_path_boundary() {
        assert_eq!(
            shorten_home("/home/suja", Some("/home/suja/")),
            TreePath {
                tilde: true,
                rest: ""
            }
        );
        assert_eq!(
            shorten_home("/home/sujata/x", Some("/home/suja")),
            TreePath {
                tilde: false,
                rest: "/home/sujata/x"
            }
        );
        assert!(!shorten_home("/x", None).tilde);
    }

    #[test]
    fn focus_highlights_the_deepest_visible_row() {
        let snapshot = snapshot();
        let rows = tree_rows(&snapshot, &HashSet::new(), None);
        assert_eq!(focused_row_index(&rows), Some(2));

        let collapsed = HashSet::from(["w1:t1".to_owned()]);
        let rows = tree_rows(&snapshot, &collapsed, None);
        assert_eq!(focused_row_index(&rows), Some(1));

        let collapsed = HashSet::from(["w1".to_owned()]);
        let rows = tree_rows(&snapshot, &collapsed, None);
        assert_eq!(focused_row_index(&rows), Some(0));
    }

    #[test]
    fn rows_map_to_focus_targets_and_toggles() {
        let snapshot = snapshot();
        let rows = tree_rows(&snapshot, &HashSet::new(), None);
        assert_eq!(
            rows[0].focus_target(),
            ClientEndpointFocusTarget::Workspace("w1".to_owned())
        );
        assert_eq!(
            rows[1].focus_target(),
            ClientEndpointFocusTarget::Tab("w1:t1".to_owned())
        );
        assert_eq!(
            rows[2].focus_target(),
            ClientEndpointFocusTarget::Pane("w1:p1".to_owned())
        );
        let rect = Rect::new(0, 5, 30, 1);
        assert_eq!(toggle_rect(&rows[0], rect), Some(Rect::new(1, 5, 2, 1)));
        assert_eq!(toggle_rect(&rows[1], rect), Some(Rect::new(3, 5, 2, 1)));
        assert_eq!(toggle_rect(&rows[2], rect), None);
    }

    #[test]
    fn scroll_metrics_clamp_to_visible_rows() {
        let metrics = uniform_scroll_metrics(10, 4, 99);
        assert_eq!(metrics.max_offset_from_bottom, 6);
        assert_eq!(metrics.offset_from_bottom, 0);
        assert_eq!(metrics.viewport_rows, 4);
        let metrics = uniform_scroll_metrics(3, 4, 2);
        assert_eq!(metrics.max_offset_from_bottom, 0);
        assert_eq!(metrics.viewport_rows, 3);
    }
}
