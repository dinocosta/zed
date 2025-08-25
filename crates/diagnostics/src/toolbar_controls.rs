use std::sync::Arc;

use crate::{BufferDiagnosticsEditor, ProjectDiagnosticsEditor, ToggleDiagnosticsRefresh};
use gpui::{Context, EventEmitter, ParentElement, Render, Window};
use project::ProjectPath;
use ui::prelude::*;
use ui::{IconButton, IconButtonShape, IconName, Tooltip};
use workspace::{ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView, item::ItemHandle};

pub struct ToolbarControls {
    editor: Option<Box<dyn DiagnosticsToolbarEditor>>,
}

pub(crate) trait DiagnosticsToolbarEditor: Send + Sync {
    /// Informs the toolbar whether warnings are included in the diagnostics.
    fn include_warnings(&self, cx: &App) -> bool;
    /// Toggles whether warning diagnostics should be displayed by the
    /// diagnostics editor.
    fn toggle_warnings(&self, window: &mut Window, cx: &mut App);
    /// Indicates whether any of the excerpts displayed by the diagnostics
    /// editor are stale.
    fn has_stale_excerpts(&self, cx: &App) -> bool;
    /// Indicates whether the diagnostics editor is currently updating the
    /// diagnostics.
    fn is_updating(&self, cx: &App) -> bool;
    /// To be deprecated, as cargo-specific details of the diagnostics
    /// implementations are being removed.
    fn cargo_diagnostics_sources(&self, cx: &App) -> Vec<ProjectPath>;
    /// Requests that the diagnostics editor stop updating the diagnostics.
    fn stop_updating(&self, cx: &mut App);
    /// Requests that the diagnostics editor updates the displayed diagnostics
    /// with the latest information.
    fn refresh_diagnostics(
        &self,
        cargo_diagnostics_sources: Arc<Vec<ProjectPath>>,
        window: &mut Window,
        cx: &mut App,
    );
    /// Returns a list of diagnostics for the provided buffer id.
    fn get_diagnostics_for_buffer(
        &self,
        buffer_id: text::BufferId,
        cx: &App,
    ) -> Vec<language::DiagnosticEntry<text::Anchor>>;
}

impl Render for ToolbarControls {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut has_stale_excerpts = false;
        let mut include_warnings = false;
        let mut is_updating = false;
        let mut cargo_diagnostics_sources = Vec::new();

        match &self.editor {
            Some(editor) => {
                include_warnings = editor.include_warnings(cx);
                has_stale_excerpts = editor.has_stale_excerpts(cx);
                cargo_diagnostics_sources = editor.cargo_diagnostics_sources(cx);
                is_updating = editor.is_updating(cx);
            }
            None => {}
        }

        let cargo_diagnostics_sources = Arc::new(cargo_diagnostics_sources);
        let fetch_cargo_diagnostics = !cargo_diagnostics_sources.is_empty();

        let warning_tooltip = if include_warnings {
            "Exclude Warnings"
        } else {
            "Include Warnings"
        };

        let warning_color = if include_warnings {
            Color::Warning
        } else {
            Color::Muted
        };

        h_flex()
            .gap_1()
            .map(|div| {
                if is_updating {
                    div.child(
                        IconButton::new("stop-updating", IconName::Stop)
                            .icon_color(Color::Info)
                            .shape(IconButtonShape::Square)
                            .tooltip(Tooltip::for_action_title(
                                "Stop diagnostics update",
                                &ToggleDiagnosticsRefresh,
                            ))
                            .on_click(cx.listener(move |toolbar_controls, _, _, cx| {
                                match toolbar_controls.editor() {
                                    Some(editor) => {
                                        editor.stop_updating(cx);
                                        cx.notify();
                                    }
                                    None => {}
                                }
                            })),
                    )
                } else {
                    div.child(
                        IconButton::new("refresh-diagnostics", IconName::ArrowCircle)
                            .icon_color(Color::Info)
                            .shape(IconButtonShape::Square)
                            .disabled(!has_stale_excerpts && !fetch_cargo_diagnostics)
                            .tooltip(Tooltip::for_action_title(
                                "Refresh diagnostics",
                                &ToggleDiagnosticsRefresh,
                            ))
                            .on_click(cx.listener({
                                move |toolbar_controls, _, window, cx| {
                                    let cargo_diagnostics_sources =
                                        Arc::clone(&cargo_diagnostics_sources);

                                    match toolbar_controls.editor() {
                                        Some(editor) => {
                                            editor.refresh_diagnostics(
                                                Arc::clone(&cargo_diagnostics_sources),
                                                window,
                                                cx,
                                            );
                                        }
                                        None => {}
                                    }
                                }
                            })),
                    )
                }
            })
            .child(
                IconButton::new("toggle-warnings", IconName::Warning)
                    .icon_color(warning_color)
                    .shape(IconButtonShape::Square)
                    .tooltip(Tooltip::text(warning_tooltip))
                    .on_click(cx.listener(|this, _, window, cx| match &this.editor {
                        Some(editor) => editor.toggle_warnings(window, cx),
                        None => {}
                    })),
            )
    }
}

impl EventEmitter<ToolbarItemEvent> for ToolbarControls {}

impl ToolbarItemView for ToolbarControls {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        if let Some(pane_item) = active_pane_item.as_ref() {
            if let Some(editor) = pane_item.downcast::<ProjectDiagnosticsEditor>() {
                self.editor = Some(Box::new(editor.downgrade()));
                ToolbarItemLocation::PrimaryRight
            } else if let Some(editor) = pane_item.downcast::<BufferDiagnosticsEditor>() {
                self.editor = Some(Box::new(editor.downgrade()));
                ToolbarItemLocation::PrimaryRight
            } else {
                ToolbarItemLocation::Hidden
            }
        } else {
            ToolbarItemLocation::Hidden
        }
    }
}

impl Default for ToolbarControls {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolbarControls {
    pub fn new() -> Self {
        ToolbarControls { editor: None }
    }

    fn editor(&self) -> Option<&Box<dyn DiagnosticsToolbarEditor>> {
        self.editor.as_ref()
    }
}
