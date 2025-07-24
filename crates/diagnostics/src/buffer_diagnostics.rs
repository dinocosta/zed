use editor::Editor;
use editor::EditorEvent;
use editor::MultiBuffer;
use gpui::AnyElement;
use gpui::App;
use gpui::AppContext;
use gpui::Context;
use gpui::Entity;
use gpui::EventEmitter;
use gpui::FocusHandle;
use gpui::Focusable;
use gpui::InteractiveElement;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::Render;
use gpui::SharedString;
use gpui::Styled;
use gpui::Window;
use gpui::actions;
use gpui::div;
use project::DiagnosticSummary;
use project::Project;
use project::ProjectPath;
use project::project_settings::DiagnosticSeverity;
use ui::Icon;
use ui::IconName;
use ui::Label;
use ui::h_flex;
use ui::prelude::*;
use util::paths::PathExt;
use workspace::ItemHandle;
use workspace::Workspace;
use workspace::item::Item;
use workspace::item::TabContentParams;

actions!(
    diagnostics,
    [
        /// Opens the project diagnostics view for the currently focused file.
        DeployCurrentFile,
    ]
);

/// The `BufferDiagnosticsEditor` is meant to be used when dealing specifically
/// with diagnostics for a single buffer, as only the excerpts of the buffer
/// where diagnostics are available are displayed.
pub(crate) struct BufferDiagnosticsEditor {
    focus_handle: FocusHandle,
    editor: Entity<Editor>,
    /// The path for which the editor is displaying diagnostics for.
    project_path: ProjectPath,
    /// Summary of the number of warnings and errors for the path. Used to
    /// display the number of warnings and errors in the tab's content.
    summary: DiagnosticSummary,
}

impl BufferDiagnosticsEditor {
    /// Creates new instance of the `BufferDiagnosticsEditor` which can then be
    /// displayed by adding it to a pane.
    fn new(
        project_path: ProjectPath,
        project_handle: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        // TODO: Update this to eventually remove the hard-coded values.
        let summary = DiagnosticSummary {
            warning_count: 2,
            error_count: 2,
        };

        let excerpts = cx.new(|cx| MultiBuffer::new(project_handle.read(cx).capability()));
        let editor = cx.new(|cx| {
            let mut editor =
                Editor::for_multibuffer(excerpts.clone(), Some(project_handle.clone()), window, cx);
            editor.set_vertical_scroll_margin(5, cx);
            editor.disable_inline_diagnostics();
            editor.set_max_diagnostics_severity(DiagnosticSeverity::Warning, cx);
            editor.set_all_diagnostics_active(cx);
            editor
        });

        Self {
            focus_handle,
            editor,
            project_path,
            summary,
        }
    }

    fn deploy(
        workspace: &mut Workspace,
        _: &DeployCurrentFile,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        // Determine the currently opened path by finding the active editor and
        // finding the project path for the buffer.
        // If there's no active editor with a project path, avoiding deploying
        // the buffer diagnostics view.
        if let Some(project_path) = workspace
            .active_item_as::<Editor>(cx)
            .map_or(None, |editor| editor.project_path(cx))
        {
            // Check if there's already a `BufferDiagnosticsEditor` tab for this
            // same path, and if so, focus on that one instead of creating a new
            // one.
            let existing_editor = workspace
                .items_of_type::<BufferDiagnosticsEditor>(cx)
                .find(|editor| editor.read(cx).project_path == project_path);

            if let Some(editor) = existing_editor {
                workspace.activate_item(&editor, true, true, window, cx);
            } else {
                let item =
                    cx.new(|cx| Self::new(project_path, workspace.project().clone(), window, cx));
                workspace.add_item_to_active_pane(Box::new(item), None, true, window, cx);
            }
        }
    }

    pub fn register(
        workspace: &mut Workspace,
        _window: Option<&mut Window>,
        _: &mut Context<Workspace>,
    ) {
        workspace.register_action(Self::deploy);
    }
}

impl Focusable for BufferDiagnosticsEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<EditorEvent> for BufferDiagnosticsEditor {}

impl Item for BufferDiagnosticsEditor {
    type Event = EditorEvent;

    // Builds the content to be displayed in the tab.
    fn tab_content(&self, params: TabContentParams, _window: &Window, _app: &App) -> AnyElement {
        let error_count = self.summary.error_count;
        let warning_count = self.summary.warning_count;
        let label = Label::new(self.project_path.path.to_sanitized_string());

        h_flex()
            .gap_1()
            .child(label.color(params.text_color()))
            .when(error_count == 0 && warning_count == 0, |parent| {
                parent.child(
                    h_flex()
                        .gap_1()
                        .child(Icon::new(IconName::Check).color(Color::Success)),
                )
            })
            .when(error_count > 0, |parent| {
                parent.child(
                    h_flex()
                        .gap_1()
                        .child(Icon::new(IconName::XCircle).color(Color::Error))
                        .child(Label::new(error_count.to_string()).color(params.text_color())),
                )
            })
            .when(warning_count > 0, |parent| {
                parent.child(
                    h_flex()
                        .gap_1()
                        .child(Icon::new(IconName::Warning).color(Color::Warning))
                        .child(Label::new(warning_count.to_string()).color(params.text_color())),
                )
            })
            .into_any_element()
    }

    fn tab_content_text(&self, _detail: usize, _app: &App) -> SharedString {
        "Buffer Diagnostics".into()
    }

    fn tab_tooltip_text(&self, _: &App) -> Option<SharedString> {
        Some(
            format!(
                "Buffer Diagnostics - {}",
                self.project_path.path.to_sanitized_string()
            )
            .into(),
        )
    }
}

impl Render for BufferDiagnosticsEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let error_count = &self.summary.error_count;
        let warning_count = &self.summary.warning_count;

        // No excerpts to be displayed.
        if error_count + warning_count == 0 {
            let label = format!(
                "No problems in {}",
                self.project_path.path.to_sanitized_string()
            );

            v_flex()
                .key_context("EmptyPane")
                .size_full()
                .gap_1()
                .justify_center()
                .items_center()
                .text_center()
                .bg(cx.theme().colors().editor_background)
                .child(Label::new(label).color(Color::Muted))
        } else {
            div().size_full().child(self.editor.clone())
        }
    }
}
