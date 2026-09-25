//! Tree presentation of the expanded sidebar: Project → Tab → Pane.
//!
//! Rows are built from the client snapshot only. Building borrows snapshot strings and the tree
//! is always fully expanded, so a frame costs one pass over the snapshot and no filesystem or
//! process work.

use std::collections::HashMap;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};

use super::render::{display_width, put_text, ShellRenderState};
use super::*;
use crate::api::schema::AgentStatus;
use crate::protocol::{ClientShellAgent, ClientShellPane};

/// Column of the project dot. Tab branches hang from it, and pane branches hang from the tab's
/// status dot two columns to the right, so every tree line drops from the middle of its parent's
/// status mark.
const TREE_INDENT: u16 = 1;
const FALLBACK_PANE_LABEL: &str = "shell";
const TAB_MARK: &str = "■";
const PROJECT_MARK: &str = "●";
const PROJECT_RULE: &str = "─";
const IDLE_TAB_MARK: &str = "·";
const PENDING_PANE_MARK: &str = "○";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TreeRowKind {
    Project,
    Tab,
    Pane,
    /// Blank row that separates one project from the next.
    Gap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TreeRow<'a> {
    pub(super) kind: TreeRowKind,
    /// Workspace, tab, or pane ID.
    pub(super) id: &'a str,
    pub(super) label: &'a str,
    /// Agent status of a pane, or the rolled-up status of a tab's or project's panes. `None`
    /// when no agent below has a known state.
    pub(super) status: Option<AgentStatus>,
    pub(super) focused: bool,
    pub(super) last_child: bool,
    /// For pane rows: whether the pane's tab is the project's last tab, which ends the project's
    /// tree line.
    pub(super) parent_last: bool,
    /// What occupies a pane row. `None` for project, tab, and gap rows.
    pub(super) icon: Option<PaneIcon>,
}

/// Icon before a pane label, so Claude panes and plain terminals are told apart at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PaneIcon {
    Claude,
    OtherAgent,
    Terminal,
}

impl PaneIcon {
    fn of(agent: Option<&ClientShellAgent>) -> Self {
        match agent {
            None => Self::Terminal,
            Some(agent) if agent.agent.as_deref() == Some("claude") => Self::Claude,
            Some(_) => Self::OtherAgent,
        }
    }

    pub(super) fn glyph(self) -> &'static str {
        match self {
            Self::Claude => "✻",
            Self::OtherAgent => "◆",
            Self::Terminal => ">_",
        }
    }

    fn color(self, palette: &Palette) -> Color {
        match self {
            Self::Claude => palette.peach,
            Self::OtherAgent => palette.overlay1,
            Self::Terminal => palette.overlay0,
        }
    }
}

/// Columns an agent pane's icon takes with its trailing space. A terminal's two-column `>_` starts
/// in the status-dot column instead, so its label still lands in the same column.
const PANE_ICON_WIDTH: u16 = 2;

impl TreeRow<'_> {
    pub(super) fn focus_target(&self) -> Option<ClientEndpointFocusTarget> {
        let id = self.id.to_owned();
        match self.kind {
            TreeRowKind::Project => Some(ClientEndpointFocusTarget::Workspace(id)),
            TreeRowKind::Tab => Some(ClientEndpointFocusTarget::Tab(id)),
            TreeRowKind::Pane => Some(ClientEndpointFocusTarget::Pane(id)),
            TreeRowKind::Gap => None,
        }
    }
}

/// A pane's own label wins, then the agent's name or kind. The live terminal title is never shown:
/// Claude Code retitles its session as the topic drifts, so the first session title is locked in
/// as the pane's label instead (see [`ClientShellState::lock_first_session_titles`]).
fn pane_label<'a>(pane: &'a ClientShellPane, agent: Option<&'a ClientShellAgent>) -> &'a str {
    pane.label
        .as_deref()
        .or_else(|| agent.and_then(|agent| agent.name.as_deref()))
        .or_else(|| agent.and_then(|agent| agent.agent.as_deref()))
        .or_else(|| agent.and_then(|agent| agent.display_agent.as_deref()))
        .filter(|label| !label.trim().is_empty())
        .unwrap_or(FALLBACK_PANE_LABEL)
}

impl ClientShellState {
    /// Names each agent pane once, after the first session title its agent sets (Claude Code
    /// titles its terminal from the user's first instruction). The name is a real `pane.rename`,
    /// so it persists with the session and later retitles never change what the tree shows. A
    /// pane is settled at most once per server boot: panes that already carry a name are settled
    /// as they are, so clearing a name does not bring the automatic one back.
    ///
    /// Tree mode only; the default sidebar never renames panes.
    pub(crate) fn lock_first_session_titles(&mut self) -> ClientShellInput {
        let mut outcome = ClientShellInput::default();
        if self.config.sidebar_mode != crate::config::SidebarModeConfig::Tree
            || !self.endpoint_is_online(&self.active_endpoint_id)
        {
            return outcome;
        }
        let probe = crate::api::schema::Method::PaneRename(crate::api::schema::PaneRenameParams {
            pane_id: String::new(),
            label: None,
        });
        if !self.supports_endpoint_method(&probe) {
            return outcome;
        }
        let Some(snapshot) = self.snapshot.as_deref() else {
            return outcome;
        };
        let boot_id = snapshot.boot_id.as_str();
        self.settled_pane_names.retain(|(boot, pane_id)| {
            boot == boot_id && snapshot.panes.iter().any(|pane| &pane.pane_id == pane_id)
        });
        let mut renames = Vec::new();
        for agent in &snapshot.agents {
            let Some(pane) = snapshot
                .panes
                .iter()
                .find(|pane| pane.pane_id == agent.pane_id)
            else {
                continue;
            };
            let named = pane
                .label
                .as_deref()
                .is_some_and(|label| !label.trim().is_empty());
            let title = session_title(agent);
            if !named && title.is_none() {
                continue;
            }
            if !self
                .settled_pane_names
                .insert((boot_id.to_owned(), pane.pane_id.clone()))
            {
                continue;
            }
            if let (false, Some(title)) = (named, title) {
                renames.push((pane.pane_id.clone(), title.to_owned()));
            }
        }
        for (pane_id, title) in renames {
            self.push_endpoint_method(
                crate::api::schema::Method::PaneRename(crate::api::schema::PaneRenameParams {
                    pane_id,
                    label: Some(title),
                }),
                &mut outcome,
            );
        }
        outcome
    }
}

/// Terminal title an agent set for its session, skipping the generic title shown before the user
/// has asked anything.
fn session_title(agent: &ClientShellAgent) -> Option<&str> {
    let title = agent.terminal_title_stripped.as_deref()?.trim();
    let generic = title.is_empty()
        || title.eq_ignore_ascii_case("claude")
        || title.eq_ignore_ascii_case("claude code")
        || agent
            .agent
            .as_deref()
            .is_some_and(|kind| title.eq_ignore_ascii_case(kind));
    (!generic).then_some(title)
}

/// Builds the visible tree rows in snapshot order.
pub(super) fn tree_rows(snapshot: &ClientShellSnapshot) -> Vec<TreeRow<'_>> {
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

    let mut rows =
        Vec::with_capacity(snapshot.workspaces.len() + snapshot.tabs.len() + snapshot.panes.len());
    for (workspace_index, workspace) in snapshot.workspaces.iter().enumerate() {
        if workspace_index > 0 {
            rows.push(TreeRow {
                kind: TreeRowKind::Gap,
                id: "",
                label: "",
                status: None,
                focused: false,
                last_child: false,
                parent_last: false,
                icon: None,
            });
        }
        let project_row = rows.len();
        rows.push(TreeRow {
            kind: TreeRowKind::Project,
            id: &workspace.workspace_id,
            label: &workspace.label,
            status: None,
            focused: workspace.focused,
            last_child: false,
            parent_last: false,
            icon: None,
        });
        let mut tab_statuses = Vec::new();
        let tabs = snapshot
            .tabs
            .iter()
            .filter(|tab| tab.workspace_id == workspace.workspace_id)
            .collect::<Vec<_>>();
        for (tab_index, tab) in tabs.iter().enumerate() {
            let last_tab = tab_index + 1 == tabs.len();
            let panes = panes_by_tab
                .get(tab.tab_id.as_str())
                .map(Vec::as_slice)
                .unwrap_or_default();
            let tab_row = rows.len();
            rows.push(TreeRow {
                kind: TreeRowKind::Tab,
                id: &tab.tab_id,
                label: &tab.label,
                status: None,
                focused: tab.focused,
                last_child: last_tab,
                parent_last: false,
                icon: None,
            });
            for (index, pane) in panes.iter().enumerate() {
                let agent = agents.get(pane.pane_id.as_str()).copied();
                rows.push(TreeRow {
                    kind: TreeRowKind::Pane,
                    id: &pane.pane_id,
                    label: pane_label(pane, agent),
                    status: agent
                        .map(|agent| agent.agent_status)
                        .filter(|status| *status != AgentStatus::Unknown),
                    focused: pane.focused,
                    last_child: index + 1 == panes.len(),
                    parent_last: last_tab,
                    icon: Some(PaneIcon::of(agent)),
                });
            }
            let tab_status = rollup_status(rows[tab_row + 1..].iter().map(|row| row.status));
            rows[tab_row].status = tab_status;
            tab_statuses.push(tab_status);
        }
        rows[project_row].status = rollup_status(tab_statuses);
    }
    rows
}

/// Status shown for a tab or project: blocked wins because it needs the user, then working,
/// then idle, then done.
pub(super) fn rollup_status(
    statuses: impl IntoIterator<Item = Option<AgentStatus>>,
) -> Option<AgentStatus> {
    let rank = |status: AgentStatus| match status {
        AgentStatus::Blocked => 4,
        AgentStatus::Working => 3,
        AgentStatus::Idle => 2,
        AgentStatus::Done => 1,
        AgentStatus::Unknown => 0,
    };
    statuses
        .into_iter()
        .flatten()
        .filter(|status| *status != AgentStatus::Unknown)
        .max_by_key(|status| rank(*status))
}

/// The deepest row on the focus path.
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
            TreeRowKind::Tab | TreeRowKind::Pane | TreeRowKind::Gap => {}
        }
    }
    focused
}

/// Status dot drawn before a pane label. `None` for unknown status.
pub(super) fn status_mark(status: AgentStatus) -> Option<&'static str> {
    match status {
        AgentStatus::Working | AgentStatus::Blocked => Some("●"),
        AgentStatus::Idle | AgentStatus::Done => Some("○"),
        AgentStatus::Unknown => None,
    }
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

    let rows = tree_rows(snapshot);
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
        if row.kind == TreeRowKind::Gap {
            continue;
        }
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

        if row.kind == TreeRowKind::Project {
            hits.workspaces.push(WorkspaceHit {
                rect,
                endpoint_id: ClientEndpointId::Local,
                workspace_id: row.id.to_owned(),
                indented: false,
                group_toggle: None,
            });
        } else if let Some(target) = row.focus_target() {
            hits.tree_targets.push((rect, target));
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
    // The collapse toggle shares the footer row, so place it first and keep the launcher clear
    // of it.
    hits.sidebar_toggle = Rect::new(
        area.right().saturating_sub(2),
        area.bottom().saturating_sub(1),
        u16::from(area.width > 1),
        u16::from(area.height > 0),
    );
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
        let launcher_text = if attention { "● menu" } else { "menu" };
        let launcher_width = display_width(launcher_text);
        // One blank column between the launcher and the toggle.
        let launcher_right = if hits.sidebar_toggle.width > 0 {
            hits.sidebar_toggle.x.saturating_sub(1)
        } else {
            content.right()
        }
        .min(content.right());
        if launcher_right > content.x + label_width + launcher_width {
            hits.global_launcher =
                Rect::new(launcher_right - launcher_width, footer_y, launcher_width, 1);
            put_text(
                buffer,
                hits.global_launcher.x,
                footer_y,
                launcher_width,
                launcher_text,
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
    let mut x = rect.x.saturating_add(TREE_INDENT);
    if x >= right {
        return;
    }

    let muted = Style::default().fg(palette.overlay0);
    let branch = |last: bool| if last { "└ " } else { "├ " };
    match row.kind {
        TreeRowKind::Project => {}
        TreeRowKind::Gap => return,
        TreeRowKind::Tab => {
            x = render::put_segment(buffer, x, rect.y, right, branch(row.last_child), muted);
        }
        TreeRowKind::Pane => {
            let rail = if row.parent_last { "  " } else { "│ " };
            x = render::put_segment(buffer, x, rect.y, right, rail, muted);
            x = render::put_segment(buffer, x, rect.y, right, branch(row.last_child), muted);
        }
    }
    // Status mark sits right after the branch. A pane without one starts its icon in that column
    // instead, so a terminal's `>_` sits under an agent pane's dot rather than behind a gap, and
    // every pane label still starts in the same column.
    let label_x = x.saturating_add(2 + PANE_ICON_WIDTH).min(right);
    if let Some((mark, color)) = row_mark(row, palette) {
        render::put_segment(buffer, x, rect.y, right, mark, Style::default().fg(color));
        x = x.saturating_add(2).min(right);
    }
    if row.kind == TreeRowKind::Tab {
        // Status comes first as on every row; the square after it only marks the row as a tab.
        x = render::put_segment(
            buffer,
            x,
            rect.y,
            right,
            TAB_MARK,
            label_style(row, palette),
        );
        x = render::put_segment(buffer, x, rect.y, right, " ", muted);
    }
    if let Some(icon) = row.icon {
        render::put_segment(
            buffer,
            x,
            rect.y,
            right,
            icon.glyph(),
            Style::default().fg(icon.color(palette)),
        );
        x = label_x;
    }

    // Long labels, such as session titles, end in `…` instead of being cut mid-word silently.
    let label = crate::ui::truncate_end(row.label, usize::from(right.saturating_sub(x)));
    x = render::put_segment(buffer, x, rect.y, right, &label, label_style(row, palette));

    // A rule after the project name, on the name's own row and in its color, sets each project
    // apart.
    if row.kind == TreeRowKind::Project && x.saturating_add(2) < right {
        let start = x + 1;
        let rule = PROJECT_RULE.repeat(usize::from(right - start));
        let style = Style::default().fg(palette.blue);
        render::put_segment(buffer, start, rect.y, right, &rule, style);
    }
}

/// Status mark before a label. Projects always show a dot, grey without a status; tabs show a small
/// grey `·` without one; panes show one only when they have a status. A tab's square is drawn
/// separately, in the tab color.
fn row_mark(row: &TreeRow<'_>, palette: &Palette) -> Option<(&'static str, Color)> {
    let status = row.status.filter(|status| *status != AgentStatus::Unknown);
    match row.kind {
        // Projects always show a dot, grey without a status, so tab branches have a mark to
        // hang from.
        TreeRowKind::Project => Some(status.map_or((PROJECT_MARK, palette.overlay0), |status| {
            (
                status_mark(status).unwrap_or(PROJECT_MARK),
                status_color(status, palette),
            )
        })),
        // A tab with no agent shows a small grey dot, so its label lines up without a gap.
        TreeRowKind::Tab => Some(status.map_or((IDLE_TAB_MARK, palette.overlay0), |status| {
            (
                status_mark(status).unwrap_or(IDLE_TAB_MARK),
                status_color(status, palette),
            )
        })),
        TreeRowKind::Pane => match status {
            Some(status) => Some((status_mark(status)?, status_color(status, palette))),
            // An agent whose state isn't known yet (Herdr waits a few seconds after startup) gets
            // a grey ring, so its icon doesn't shift right when the real status arrives.
            None if row.icon.is_some_and(|icon| icon != PaneIcon::Terminal) => {
                Some((PENDING_PANE_MARK, palette.overlay0))
            }
            None => None,
        },
        TreeRowKind::Gap => None,
    }
}

/// Label color by row kind. Focus is shown only by the row highlight, so a label looks the same
/// on and off the focus path.
pub(super) fn label_style(row: &TreeRow<'_>, palette: &Palette) -> Style {
    match row.kind {
        TreeRowKind::Project => Style::default()
            .fg(palette.blue)
            .add_modifier(Modifier::BOLD),
        TreeRowKind::Tab => Style::default()
            .fg(palette.mauve)
            .add_modifier(Modifier::BOLD),
        // Lighter than the overlay0 tree lines so pane names stay readable.
        TreeRowKind::Pane | TreeRowKind::Gap => Style::default().fg(palette.subtext0),
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
                let branch = |last: bool| if last { "└ " } else { "├ " };
                let indent = match row.kind {
                    TreeRowKind::Project => String::new(),
                    TreeRowKind::Tab => branch(row.last_child).to_owned(),
                    TreeRowKind::Pane => format!(
                        "{}{}",
                        if row.parent_last { "  " } else { "│ " },
                        branch(row.last_child)
                    ),
                    TreeRowKind::Gap => return String::new(),
                };
                let status = match row.kind {
                    TreeRowKind::Tab => format!(
                        "{} {TAB_MARK} ",
                        row.status.and_then(status_mark).unwrap_or(IDLE_TAB_MARK)
                    ),
                    TreeRowKind::Project => format!(
                        "{} ",
                        row.status.and_then(status_mark).unwrap_or(PROJECT_MARK)
                    ),
                    _ => row
                        .status
                        .and_then(status_mark)
                        .map(|dot| format!("{dot} "))
                        .unwrap_or_default(),
                };
                let icon = row
                    .icon
                    .map(|icon| {
                        // Labels line up whether or not a status dot precedes the icon.
                        let width = if status.is_empty() { 4 } else { 2 };
                        format!("{:<width$}", icon.glyph())
                    })
                    .unwrap_or_default();
                format!("{indent}{status}{icon}{}", row.label)
            })
            .collect()
    }

    #[test]
    fn open_tree_lists_projects_tabs_and_panes_in_snapshot_order() {
        let snapshot = snapshot();
        let rows = tree_rows(&snapshot);
        assert_eq!(
            outline(&rows),
            [
                "● Job search",
                "├ ● ■ 1",
                "│ ├ ● ✻ claude",
                "│ └ >_  bash",
                "└ · ■ 2",
                "  ├ >_  shell",
                "  └ >_  shell",
                "",
                "○ OpenMRS",
                "└ ○ ■ 1",
                "  └ ○ ✻ claude",
            ]
        );
        assert!(!rows[2].last_child);
        assert!(rows[3].last_child);
        assert_eq!(rows[0].status, Some(AgentStatus::Working));
        assert_eq!(rows[4].status, None, "a tab without agents has no dot");
    }

    #[test]
    fn pane_label_ignores_the_live_session_title() {
        let mut snapshot = snapshot();
        snapshot.agents[0].terminal_title_stripped = Some("Resume handoff instructions".into());
        snapshot.agents[1].terminal_title_stripped = Some("Claude Code".into());
        let rows = tree_rows(&snapshot);
        assert_eq!(
            rows[2].label, "claude",
            "the session title only shows once it is locked in as the pane's label"
        );
        assert_eq!(rows[10].label, "claude");

        // A pane label (user-given or the locked session title) wins.
        snapshot.panes[0].label = Some("Resume handoff instructions".into());
        let rows = tree_rows(&snapshot);
        assert_eq!(rows[2].label, "Resume handoff instructions");
    }

    #[test]
    fn session_title_skips_generic_titles() {
        let mut agent = agent("w1:p1", AgentStatus::Idle);
        for generic in ["Claude Code", "claude", "CLAUDE", "  "] {
            agent.terminal_title_stripped = Some(generic.into());
            assert_eq!(session_title(&agent), None, "{generic:?}");
        }
        agent.terminal_title_stripped = Some("  Resume handoff  ".into());
        assert_eq!(session_title(&agent), Some("Resume handoff"));
    }

    #[test]
    fn rollup_prefers_blocked_then_working_then_idle_then_done() {
        use AgentStatus::*;
        assert_eq!(
            rollup_status([Some(Done), Some(Blocked), Some(Working)]),
            Some(Blocked)
        );
        assert_eq!(
            rollup_status([Some(Idle), Some(Working), None]),
            Some(Working)
        );
        assert_eq!(rollup_status([Some(Done), Some(Idle)]), Some(Idle));
        assert_eq!(rollup_status([Some(Done), None]), Some(Done));
        assert_eq!(rollup_status([None, Some(Unknown)]), None);
        assert_eq!(rollup_status([]), None);
    }

    #[test]
    fn project_rolls_up_across_tabs() {
        let mut snapshot = snapshot();
        // Tab 1 is working; make a tab 2 pane blocked, which must win at project level.
        snapshot.agents.push(agent("w1:p3", AgentStatus::Blocked));
        let rows = tree_rows(&snapshot);
        assert_eq!(rows[0].status, Some(AgentStatus::Blocked));
        assert_eq!(rows[1].status, Some(AgentStatus::Working));
        assert_eq!(rows[4].status, Some(AgentStatus::Blocked));
    }

    #[test]
    fn status_marks_follow_agent_state_and_skip_unknown() {
        assert_eq!(status_mark(AgentStatus::Working), Some("●"));
        assert_eq!(status_mark(AgentStatus::Blocked), Some("●"));
        assert_eq!(status_mark(AgentStatus::Idle), Some("○"));
        assert_eq!(status_mark(AgentStatus::Done), Some("○"));
        assert_eq!(status_mark(AgentStatus::Unknown), None);

        let mut snapshot = snapshot();
        snapshot.agents[0].agent_status = AgentStatus::Unknown;
        let rows = tree_rows(&snapshot);
        assert_eq!(rows[2].status, None);
    }

    #[test]
    fn labels_are_colored_by_kind() {
        let snapshot = snapshot();
        let rows = tree_rows(&snapshot);
        let palette = Palette::catppuccin();
        let project = Style::default()
            .fg(palette.blue)
            .add_modifier(Modifier::BOLD);
        let tab = Style::default()
            .fg(palette.mauve)
            .add_modifier(Modifier::BOLD);
        let pane = Style::default().fg(palette.subtext0);

        // Focus path: project "Job search", tab 1, pane "claude".
        assert_eq!(label_style(&rows[0], &palette), project);
        assert_eq!(label_style(&rows[1], &palette), tab);
        assert_eq!(label_style(&rows[2], &palette), pane);

        // Off the focus path: identical styles, no dimming.
        assert_eq!(label_style(&rows[3], &palette), pane, "unfocused pane");
        assert_eq!(label_style(&rows[4], &palette), tab, "unfocused tab");
        assert_eq!(
            label_style(&rows[8], &palette),
            project,
            "unfocused project"
        );
    }

    #[test]
    fn focus_highlights_the_deepest_focused_row() {
        let snapshot = snapshot();
        let rows = tree_rows(&snapshot);
        assert_eq!(focused_row_index(&rows), Some(2));
    }

    #[test]
    fn rows_map_to_focus_targets() {
        let snapshot = snapshot();
        let rows = tree_rows(&snapshot);
        assert_eq!(
            rows[0].focus_target(),
            Some(ClientEndpointFocusTarget::Workspace("w1".to_owned()))
        );
        assert_eq!(
            rows[1].focus_target(),
            Some(ClientEndpointFocusTarget::Tab("w1:t1".to_owned()))
        );
        assert_eq!(
            rows[2].focus_target(),
            Some(ClientEndpointFocusTarget::Pane("w1:p1".to_owned()))
        );
        assert_eq!(rows[7].kind, TreeRowKind::Gap);
        assert_eq!(rows[7].focus_target(), None);
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
