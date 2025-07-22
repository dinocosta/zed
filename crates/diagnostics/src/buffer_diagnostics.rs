// use anyhow::Result;
// use editor::Editor;
// use editor::MultiBuffer;
// use editor::display_map::CustomBockId;
// use gpui::Entity;
// use gpui::Task;
// use gpui::WeakEntity;
use editor::EditorEvent;
use gpui::AnyElement;
use gpui::App;
use gpui::AppContext;
use gpui::Context;
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
use ui::Label;
use ui::h_flex;
use workspace::Workspace;
use workspace::item::Item;
use workspace::item::TabContentParams;
// use language::DiagnosticEntry;
// use project::DiagnosticSummary;
// use project::Project;
// use text::Anchor;
// use text::BufferId;

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
}

impl BufferDiagnosticsEditor {
    /// Creates new instance of the `BufferDiagnosticsEditor` which can then be
    /// displayed by adding it to a pane.
    fn new(cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();

        Self { focus_handle }
    }

    fn deploy(
        workspace: &mut Workspace,
        _: &DeployCurrentFile,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let item = cx.new(|cx| Self::new(cx));
        workspace.add_item_to_active_pane(Box::new(item), None, true, window, cx);
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
    fn tab_content(&self, _params: TabContentParams, _window: &Window, _app: &App) -> AnyElement {
        h_flex()
            .gap_1()
            .child(Label::new("Buffer Diagnostics"))
            .into_any_element()
    }

    // Builds the content to be displayed in the tab.
    fn tab_content_text(&self, _detail: usize, _app: &App) -> SharedString {
        "Buffer Diagnostics".into()
    }
}

impl Render for BufferDiagnosticsEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("Diagnostics")
            .track_focus(&self.focus_handle(cx))
            .size_full()
    }
}
