use crate::CargoDiagnosticsFetchState;
use crate::DIAGNOSTICS_SUMMARY_UPDATE_DELAY;
use crate::DIAGNOSTICS_UPDATE_DELAY;
use crate::IncludeWarnings;
use crate::ToggleWarnings;
use crate::context_range_for_entry;
use crate::diagnostic_renderer::DiagnosticBlock;
use crate::diagnostic_renderer::DiagnosticRenderer;
use crate::diagnostic_renderer::DiagnosticsEditor;
use anyhow::Result;
use collections::HashMap;
use editor::DEFAULT_MULTIBUFFER_CONTEXT;
use editor::Editor;
use editor::EditorEvent;
use editor::ExcerptRange;
use editor::MultiBuffer;
use editor::PathKey;
use editor::display_map::BlockPlacement;
use editor::display_map::BlockProperties;
use editor::display_map::BlockStyle;
use editor::display_map::CustomBlockId;
use futures::future::join_all;
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
use gpui::Subscription;
use gpui::Task;
use gpui::Window;
use gpui::actions;
use gpui::div;
use language::Buffer;
use language::BufferId;
use language::DiagnosticEntry;
use language::Point;
use project::DiagnosticSummary;
use project::Event;
use project::Project;
use project::ProjectPath;
use project::lsp_store::rust_analyzer_ext::cancel_flycheck;
use project::lsp_store::rust_analyzer_ext::run_flycheck;
use project::project_settings::DiagnosticSeverity;
use project::project_settings::ProjectSettings;
use settings::Settings;
use std::cmp::Ordering;
use std::sync::Arc;
use text::Anchor;
use text::BufferSnapshot;
use text::OffsetRangeExt;
use ui::Icon;
use ui::IconName;
use ui::Label;
use ui::h_flex;
use ui::prelude::*;
use util::ResultExt;
use util::paths::PathExt;
use workspace::ItemHandle;
use workspace::ToolbarItemLocation;
use workspace::Workspace;
use workspace::item::BreadcrumbText;
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
    pub project: Entity<Project>,
    focus_handle: FocusHandle,
    editor: Entity<Editor>,
    /// The current diagnostic entries in the `BufferDiagnosticsEditor`. Used to
    /// allow quick comparison of updated diagnostics, to confirm if anything
    /// has changed.
    diagnostics: Vec<DiagnosticEntry<Anchor>>,
    /// The blocks used to display the diagnostics' content in the editor, next
    /// to the excerpts where the diagnostic originated.
    blocks: Vec<CustomBlockId>,
    /// Multibuffer to contain all excerpts that contain diagnostics, which are
    /// to be rendered in the editor.
    multibuffer: Entity<MultiBuffer>,
    /// The path for which the editor is displaying diagnostics for.
    project_path: ProjectPath,
    /// Summary of the number of warnings and errors for the path. Used to
    /// display the number of warnings and errors in the tab's content.
    summary: DiagnosticSummary,
    /// Whether to include warnings in the list of diagnostics shown in the
    /// editor.
    pub include_warnings: bool,
    /// Keeps track of whether there's a background task already running to
    /// update the excerpts, in order to avoid firing multiple tasks for this purpose.
    pub update_excerpts_task: Option<Task<Result<()>>>,
    /// Keeps track of the task responsible for updating the
    /// `BufferDiagnosticsEditor`'s diagnostic summary.
    diagnostic_summary_task: Task<()>,
    /// Tracks the state of fetching cargo diagnostics, including any running
    /// fetch tasks and the diagnostic sources being processed.
    pub cargo_diagnostics_fetch: CargoDiagnosticsFetchState,
    /// The project's subscription, responsible for processing events related to
    /// diagnostics.
    _subscription: Subscription,
}

impl BufferDiagnosticsEditor {
    /// Creates new instance of the `BufferDiagnosticsEditor` which can then be
    /// displayed by adding it to a pane.
    fn new(
        project_path: ProjectPath,
        project_handle: Entity<Project>,
        include_warnings: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Subscribe to project events related to diagnostics so the
        // `BufferDiagnosticsEditor` can update its state accordingly.
        let project_event_subscription = cx.subscribe_in(
            &project_handle,
            window,
            |buffer_diagnostics_editor, _project, event, window, cx| match event {
                Event::DiskBasedDiagnosticsStarted { .. } => {
                    cx.notify();
                }
                Event::DiskBasedDiagnosticsFinished { .. } => {
                    buffer_diagnostics_editor.update_stale_excerpts(window, cx);
                }
                Event::DiagnosticsUpdated {
                    path,
                    language_server_id,
                } => {
                    // When diagnostics have been updated, the
                    // `BufferDiagnosticsEditor` should update its state only if
                    // the path matches its `project_path`, otherwise the event should be ignored.
                    if *path == buffer_diagnostics_editor.project_path {
                        // Start a task to update the diagnostic summary.
                        buffer_diagnostics_editor.diagnostic_summary_task =
                            cx.spawn(async move |buffer_diagnostics_editor, cx| {
                                cx.background_executor()
                                    .timer(DIAGNOSTICS_SUMMARY_UPDATE_DELAY)
                                    .await;

                                buffer_diagnostics_editor
                                    .update(cx, |buffer_diagnostics_editor, cx| {
                                        buffer_diagnostics_editor.update_diagnostic_summary(cx);
                                    })
                                    .log_err();
                            });

                        if buffer_diagnostics_editor.editor.focus_handle(cx).contains_focused(window, cx) || buffer_diagnostics_editor.focus_handle.contains_focused(window, cx) {
                            log::debug!("diagnostics updated for server {language_server_id}, path {path:?}. recording change");
                        } else {
                            log::debug!("diagnostics updated for server {language_server_id}, path {path:?}. updating excerpts");
                            buffer_diagnostics_editor.update_stale_excerpts(window, cx);
                        }
                    }
                }
                _ => {}
            },
        );

        // Whenever the `IncludeWarnings` setting changes, update the
        // `include_warnings` field, update the associated editor's
        // `max_diagnostics_severity` accordingly as well as the diagnostics and
        // excerpts, ensuring that the warnings are correctly included or
        // excluded from the summary and excerpts.
        cx.observe_global_in::<IncludeWarnings>(window, |buffer_diagnostics_editor, window, cx| {
            let include_warnings = cx.global::<IncludeWarnings>().0;
            let max_severity = Self::max_diagnostics_severity(include_warnings);

            buffer_diagnostics_editor.include_warnings = include_warnings;
            buffer_diagnostics_editor.editor.update(cx, |editor, cx| {
                editor.set_max_diagnostics_severity(max_severity, cx);
            });

            buffer_diagnostics_editor.diagnostics.clear();
            buffer_diagnostics_editor.update_all_diagnostics(false, window, cx);
        })
        .detach();

        let project = project_handle.clone();
        let focus_handle = cx.focus_handle();

        cx.on_focus_in(
            &focus_handle,
            window,
            |buffer_diagnostics_editor, window, cx| buffer_diagnostics_editor.focus_in(window, cx),
        )
        .detach();

        cx.on_focus_out(
            &focus_handle,
            window,
            |buffer_diagnostics_editor, _event, window, cx| {
                buffer_diagnostics_editor.focus_out(window, cx)
            },
        )
        .detach();

        let summary = project_handle
            .read(cx)
            .diagnostic_summary_for_path(&project_path, false, cx);

        let multibuffer = cx.new(|cx| MultiBuffer::new(project_handle.read(cx).capability()));
        let max_severity = Self::max_diagnostics_severity(include_warnings);
        let editor = cx.new(|cx| {
            let mut editor = Editor::for_multibuffer(
                multibuffer.clone(),
                Some(project_handle.clone()),
                window,
                cx,
            );
            editor.set_vertical_scroll_margin(5, cx);
            editor.disable_inline_diagnostics();
            editor.set_max_diagnostics_severity(max_severity, cx);
            editor.set_all_diagnostics_active(cx);
            editor
        });

        // Subscribe to events triggered by the editor in order to correctly
        // update the buffer's excerpts.
        cx.subscribe_in(
            &editor,
            window,
            |buffer_diagnostics_editor, _editor, event: &EditorEvent, window, cx| {
                cx.emit(event.clone());

                match event {
                    // If the user tries to focus on the editor but there's actually
                    // no excerpts for the buffer, focus back on the
                    // `BufferDiagnosticsEditor` instance.
                    EditorEvent::Focused => {
                        if buffer_diagnostics_editor.multibuffer.read(cx).is_empty() {
                            window.focus(&buffer_diagnostics_editor.focus_handle);
                        }
                    }
                    EditorEvent::Blurred => {
                        buffer_diagnostics_editor.update_stale_excerpts(window, cx)
                    }
                    _ => {}
                }
            },
        )
        .detach();

        let diagnostics = vec![];
        let update_excerpts_task = None;
        let diagnostic_summary_task = Task::ready(());
        let cargo_diagnostics_fetch: CargoDiagnosticsFetchState = Default::default();
        let mut buffer_diagnostics_editor = Self {
            project,
            focus_handle,
            editor,
            diagnostics,
            blocks: Default::default(),
            multibuffer,
            project_path,
            summary,
            include_warnings,
            update_excerpts_task,
            diagnostic_summary_task,
            cargo_diagnostics_fetch,
            _subscription: project_event_subscription,
        };

        buffer_diagnostics_editor.update_all_diagnostics(true, window, cx);
        buffer_diagnostics_editor
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
                let include_warnings = match cx.try_global::<IncludeWarnings>() {
                    Some(include_warnings) => include_warnings.0,
                    None => ProjectSettings::get_global(cx).diagnostics.include_warnings,
                };

                let item = cx.new(|cx| {
                    Self::new(
                        project_path,
                        workspace.project().clone(),
                        include_warnings,
                        window,
                        cx,
                    )
                });

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

    fn update_all_diagnostics(
        &mut self,
        first_launch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cargo_diagnostics_sources = self.cargo_diagnostics_sources(cx);

        if cargo_diagnostics_sources.is_empty() || (first_launch && !self.summary.is_empty()) {
            self.update_all_excerpts(window, cx);
        } else {
            self.fetch_cargo_diagnostics(Arc::new(cargo_diagnostics_sources), cx);
        }
    }

    fn update_diagnostic_summary(&mut self, cx: &mut Context<Self>) {
        let project = self.project.read(cx);

        self.summary = project.diagnostic_summary_for_path(&self.project_path, false, cx);
    }

    // TODO: Why do we need this? Is it specific for Rust projects? Can it be
    // moved to the `diagnostics` module so it can be reused by both
    // `BufferDiagnosticsEditor` and `BufferDiagnosticsEditor`?
    pub fn cargo_diagnostics_sources(&self, cx: &App) -> Vec<ProjectPath> {
        let fetch_cargo_diagnostics = ProjectSettings::get_global(cx)
            .diagnostics
            .fetch_cargo_diagnostics();

        if !fetch_cargo_diagnostics {
            return Vec::new();
        }

        self.project
            .read(cx)
            .worktrees(cx)
            .filter_map(|worktree| {
                let rust_file_entry = worktree.read(cx).entries(false, 0).find(|entry| {
                    entry
                        .path
                        .extension()
                        .and_then(|extension| extension.to_str())
                        == Some("rs")
                })?;

                self.project.read(cx).path_for_entry(rust_file_entry.id, cx)
            })
            .collect()
    }

    pub fn fetch_cargo_diagnostics(
        &mut self,
        diagnostics_sources: Arc<Vec<ProjectPath>>,
        cx: &mut Context<Self>,
    ) {
        let project = self.project.clone();
        self.cargo_diagnostics_fetch.cancel_task = None;
        self.cargo_diagnostics_fetch.fetch_task = None;
        self.cargo_diagnostics_fetch.diagnostic_sources = diagnostics_sources.clone();
        if self.cargo_diagnostics_fetch.diagnostic_sources.is_empty() {
            return;
        }

        self.cargo_diagnostics_fetch.fetch_task = Some(cx.spawn(async move |editor, cx| {
            let mut fetch_tasks = Vec::new();
            for buffer_path in diagnostics_sources.iter().cloned() {
                if cx
                    .update(|cx| {
                        fetch_tasks.push(run_flycheck(project.clone(), buffer_path, cx));
                    })
                    .is_err()
                {
                    break;
                }
            }

            let _ = join_all(fetch_tasks).await;
            editor
                .update(cx, |editor, _| {
                    editor.cargo_diagnostics_fetch.fetch_task = None;
                })
                .ok();
        }));
    }

    // TODO: Why does this one need to be public while the one on
    // `ProjectDiagnosticsEditor` is not and everything seems to be working on
    // ToolbarControls?
    pub fn stop_cargo_diagnostics_fetch(&mut self, cx: &mut App) {
        self.cargo_diagnostics_fetch.fetch_task = None;
        let mut cancel_gasks = Vec::new();
        for buffer_path in std::mem::take(&mut self.cargo_diagnostics_fetch.diagnostic_sources)
            .iter()
            .cloned()
        {
            cancel_gasks.push(cancel_flycheck(self.project.clone(), buffer_path, cx));
        }

        self.cargo_diagnostics_fetch.cancel_task = Some(cx.background_spawn(async move {
            let _ = join_all(cancel_gasks).await;
            log::info!("Finished fetching cargo diagnostics");
        }));
    }

    // TODO: Refactor this, since there's only a single project path, we can
    // probably fetch its buffer and not actually need to have
    // `update_stale_excerpts`, `update_all_excerpts` and `update_excerpts`.
    fn update_stale_excerpts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // If there's already a task updating the excerpts, early return and let
        // the other task finish.
        if self.update_excerpts_task.is_some() {
            return;
        }

        let project_handle = self.project.clone();
        let project_path = self.project_path.clone();

        self.update_excerpts_task = Some(cx.spawn_in(window, async move |editor, cx| {
            cx.background_executor()
                .timer(DIAGNOSTICS_UPDATE_DELAY)
                .await;

            let buffer = project_handle
                .update(cx, |project, cx| {
                    project.open_buffer(project_path.clone(), cx)
                })?
                .await
                .log_err();

            if let Some(buffer) = buffer {
                editor
                    .update_in(cx, |editor, window, cx| {
                        editor.update_excerpts(buffer, window, cx)
                    })?
                    .await?;

                let _ = editor.update(cx, |editor, cx| {
                    editor.update_excerpts_task = None;
                    cx.notify();
                });
            };

            Ok(())
        }));
    }

    /// Enqueue an update of all excerpts. Updates all paths that either have
    /// diagnostics or are currently present in this view to ensure that new
    /// diagnostics are added and that excerpts that are shown but no longer
    /// have diagnostics are removed from the editor.
    /// TODO: Update this to only deal with the active file path, we don't need
    /// to iterate over all paths, as this view is meant to only deal with a
    /// single file.
    /// TODO: Update this to behave like the regular `ProjectDiagnosticsEditor`
    /// and run this in a background task with `update_stale_excerpts`.
    pub fn update_all_excerpts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.update_stale_excerpts(window, cx);
    }

    /// Updates the excerpts in the `BufferDiagnosticsEditor` for a single
    /// buffer.
    fn update_excerpts(
        &mut self,
        buffer: Entity<Buffer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let was_empty = self.multibuffer.read(cx).is_empty();
        let buffer_snapshot = buffer.read(cx).snapshot();
        let buffer_snapshot_max = buffer_snapshot.max_point();
        let max_severity = Self::max_diagnostics_severity(self.include_warnings)
            .into_lsp()
            .unwrap_or(lsp::DiagnosticSeverity::WARNING);

        cx.spawn_in(window, async move |buffer_diagnostics_editor, mut cx| {
            // Fetch the diagnostics for the whole of the buffer
            // (`Point::zero()..buffer_snapshot.max_point()`) so we can confirm
            // if the diagnostics changed, if it didn't, early return as there's
            // nothing to update.
            let diagnostics = buffer_snapshot
                .diagnostics_in_range::<_, Anchor>(Point::zero()..buffer_snapshot_max, false)
                .collect::<Vec<_>>();

            let unchanged =
                buffer_diagnostics_editor.update(cx, |buffer_diagnostics_editor, _cx| {
                    if buffer_diagnostics_editor
                        .diagnostics_are_unchanged(&diagnostics, &buffer_snapshot)
                    {
                        return true;
                    }

                    buffer_diagnostics_editor.set_diagnostics(&diagnostics);
                    return false;
                })?;

            if unchanged {
                return Ok(());
            }

            // Mapping between the Group ID and a vector of DiagnosticEntry.
            let mut grouped: HashMap<usize, Vec<_>> = HashMap::default();
            for entry in diagnostics {
                grouped
                    .entry(entry.diagnostic.group_id)
                    .or_default()
                    .push(DiagnosticEntry {
                        range: entry.range.to_point(&buffer_snapshot),
                        diagnostic: entry.diagnostic,
                    })
            }

            let mut blocks: Vec<DiagnosticBlock<BufferDiagnosticsEditor>> = Vec::new();
            for (_, group) in grouped {
                // If the minimum severity of the group is higher than the
                // maximum severity, or it doesn't even have severity, skip this
                // group.
                if group
                    .iter()
                    .map(|d| d.diagnostic.severity)
                    .min()
                    .is_none_or(|severity| severity > max_severity)
                {
                    continue;
                }

                let diagnostic_blocks = cx.update(|_window, cx| {
                    DiagnosticRenderer::diagnostic_blocks_for_group(
                        group,
                        buffer_snapshot.remote_id(),
                        Some(buffer_diagnostics_editor.clone()),
                        cx,
                    )
                })?;

                // TODO: What's happening here? Is there a way to write this in
                // a cleaner way?
                for diagnostic_block in diagnostic_blocks {
                    let index = blocks
                        .binary_search_by(|probe| {
                            probe
                                .initial_range
                                .start
                                .cmp(&diagnostic_block.initial_range.start)
                                .then(
                                    probe
                                        .initial_range
                                        .end
                                        .cmp(&diagnostic_block.initial_range.end),
                                )
                                .then(Ordering::Greater)
                        })
                        .unwrap_or_else(|index| index);

                    blocks.insert(index, diagnostic_block);
                }
            }

            // Build the excerpt ranges for this specific buffer's diagnostics,
            // so those excerpts can later be used to update the excerpts shown
            // in the editor.
            // This is done by iterating over the list of diagnostic blocks and
            // determine what range does the diagnostic block span.
            let mut excerpt_ranges: Vec<ExcerptRange<Point>> = Vec::new();

            for diagnostic_block in blocks.iter() {
                let excerpt_range = context_range_for_entry(
                    diagnostic_block.initial_range.clone(),
                    DEFAULT_MULTIBUFFER_CONTEXT,
                    buffer_snapshot.clone(),
                    &mut cx,
                )
                .await;

                // TODO: Do we actually need to do this if we just did it for
                // the diagnostic blocks? Shouldn't this already be sorted?
                let index = excerpt_ranges
                    .binary_search_by(|probe| {
                        probe
                            .context
                            .start
                            .cmp(&excerpt_range.start)
                            .then(probe.context.end.cmp(&excerpt_range.end))
                            .then(
                                probe
                                    .primary
                                    .start
                                    .cmp(&diagnostic_block.initial_range.start),
                            )
                            .then(probe.primary.end.cmp(&diagnostic_block.initial_range.end))
                            .then(Ordering::Greater)
                    })
                    .unwrap_or_else(|index| index);

                excerpt_ranges.insert(
                    index,
                    ExcerptRange {
                        context: excerpt_range,
                        primary: diagnostic_block.initial_range.clone(),
                    },
                )
            }

            // Finally, update the editor's content with the new excerpt ranges
            // for this editor, as well as the diagnostic blocks.
            buffer_diagnostics_editor.update_in(cx, |buffer_diagnostics_editor, window, cx| {
                // Remove the list of `CustomBlockId` from the editor's display
                // map, ensuring that if any diagnostics have been solved, the
                // associated block stops being shown.
                let block_ids = buffer_diagnostics_editor.blocks.clone();

                buffer_diagnostics_editor.editor.update(cx, |editor, cx| {
                    editor.display_map.update(cx, |display_map, cx| {
                        display_map.remove_blocks(block_ids.into_iter().collect(), cx);
                    })
                });

                let (anchor_ranges, _) =
                    buffer_diagnostics_editor
                        .multibuffer
                        .update(cx, |multibuffer, cx| {
                            multibuffer.set_excerpt_ranges_for_path(
                                PathKey::for_buffer(&buffer, cx),
                                buffer.clone(),
                                &buffer_snapshot,
                                excerpt_ranges,
                                cx,
                            )
                        });

                // TODO: If the multibuffer was empty before the excerpt ranges
                // were updated, update the editor's selections to the first
                // excerpt range.
                if was_empty {
                    if let Some(anchor_range) = anchor_ranges.first() {
                        let range_to_select = anchor_range.start..anchor_range.start;

                        buffer_diagnostics_editor.editor.update(cx, |editor, cx| {
                            editor.change_selections(Default::default(), window, cx, |selection| {
                                selection.select_anchor_ranges([range_to_select])
                            })
                        });

                        // If the `BufferDiagnosticsEditor` is currently
                        // focused, move focus to its editor.
                        if buffer_diagnostics_editor.focus_handle.is_focused(window) {
                            buffer_diagnostics_editor
                                .editor
                                .read(cx)
                                .focus_handle(cx)
                                .focus(window);
                        }
                    }
                }

                // Build new diagnostic blocks to be added to the editor's
                // display map for the new diagnostics. Update the `blocks`
                // property before finishing, to ensure the blocks are removed
                // on the next execution.
                let editor_blocks =
                    anchor_ranges
                        .into_iter()
                        .zip(blocks.into_iter())
                        .map(|(anchor, block)| {
                            let editor = buffer_diagnostics_editor.editor.downgrade();

                            BlockProperties {
                                placement: BlockPlacement::Near(anchor.start),
                                height: Some(1),
                                style: BlockStyle::Flex,
                                render: Arc::new(move |block_context| {
                                    block.render_block(editor.clone(), block_context)
                                }),
                                priority: 1,
                            }
                        });

                let block_ids = buffer_diagnostics_editor.editor.update(cx, |editor, cx| {
                    editor.display_map.update(cx, |display_map, cx| {
                        display_map.insert_blocks(editor_blocks, cx)
                    })
                });

                buffer_diagnostics_editor.blocks = block_ids;
                cx.notify()
            })
        })
    }

    fn set_diagnostics(&mut self, diagnostics: &Vec<DiagnosticEntry<Anchor>>) {
        self.diagnostics = diagnostics.clone();
    }

    fn diagnostics_are_unchanged(
        &self,
        diagnostics: &Vec<DiagnosticEntry<Anchor>>,
        snapshot: &BufferSnapshot,
    ) -> bool {
        if self.diagnostics.len() != diagnostics.len() {
            return false;
        }

        self.diagnostics
            .iter()
            .zip(diagnostics.iter())
            .all(|(existing, new)| {
                existing.diagnostic.message == new.diagnostic.message
                    && existing.diagnostic.severity == new.diagnostic.severity
                    && existing.diagnostic.is_primary == new.diagnostic.is_primary
                    && existing.range.to_offset(snapshot) == new.range.to_offset(snapshot)
            })
    }

    fn focus_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // If the `BufferDiagnosticsEditor` is focused and the multibuffer is
        // not empty, focus on the editor instead, which will allow the user to
        // start interacting and editing the buffer's contents.
        if self.focus_handle.is_focused(window) && !self.multibuffer.read(cx).is_empty() {
            self.editor.focus_handle(cx).focus(window)
        }
    }

    fn focus_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_handle.is_focused(window) && !self.editor.focus_handle(cx).is_focused(window)
        {
            self.update_all_excerpts(window, cx);
        }
    }

    pub fn toggle_warnings(&mut self, _: &ToggleWarnings, _: &mut Window, cx: &mut Context<Self>) {
        cx.set_global(IncludeWarnings(!self.include_warnings));
    }

    fn max_diagnostics_severity(include_warnings: bool) -> DiagnosticSeverity {
        match include_warnings {
            true => DiagnosticSeverity::Warning,
            false => DiagnosticSeverity::Error,
        }
    }
}

impl DiagnosticsEditor for BufferDiagnosticsEditor {
    fn get_diagnostics_for_buffer(
        &self,
        _buffer_id: BufferId,
        _cx: &App,
    ) -> Vec<DiagnosticEntry<Anchor>> {
        // TODO: We should probably save the ID of the buffer that the buffer
        // diagnostics editor is working with, so that, if it doesn't match the
        // argument, we can return an empty vector.
        self.diagnostics.clone()
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

    fn breadcrumb_location(&self, _: &App) -> ToolbarItemLocation {
        ToolbarItemLocation::PrimaryLeft
    }

    fn breadcrumbs(&self, theme: &theme::Theme, cx: &App) -> Option<Vec<BreadcrumbText>> {
        self.editor.breadcrumbs(theme, cx)
    }

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

    fn can_save(&self, _cx: &App) -> bool {
        true
    }

    fn save(
        &mut self,
        options: workspace::item::SaveOptions,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        self.editor.save(options, project, window, cx)
    }
}

impl Render for BufferDiagnosticsEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filename = self.project_path.path.to_sanitized_string();
        let error_count = self.summary.error_count;
        let warning_count = match self.include_warnings {
            true => self.summary.warning_count,
            false => 0,
        };

        // No excerpts to be displayed.
        let child = if error_count + warning_count == 0 {
            let label = match warning_count {
                0 => format!("No problems in {}", filename),
                _ => format!("No errors in {}", filename),
            };

            v_flex()
                .key_context("EmptyPane")
                .size_full()
                .gap_1()
                .justify_center()
                .items_center()
                .text_center()
                .bg(cx.theme().colors().editor_background)
                .child(Label::new(label).color(Color::Muted))
                .when(self.summary.warning_count > 0, |div| {
                    let label = match self.summary.warning_count {
                        1 => "Show 1 warning".into(),
                        warning_count => format!("Show {} warnings", warning_count),
                    };

                    div.child(
                        Button::new("diagnostics-show-warning-label", label).on_click(cx.listener(
                            |buffer_diagnostics_editor, _, window, cx| {
                                buffer_diagnostics_editor.toggle_warnings(
                                    &Default::default(),
                                    window,
                                    cx,
                                );
                                cx.notify();
                            },
                        )),
                    )
                })
        } else {
            div().size_full().child(self.editor.clone())
        };

        div()
            .key_context("Diagnostics")
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .child(child)
    }
}
